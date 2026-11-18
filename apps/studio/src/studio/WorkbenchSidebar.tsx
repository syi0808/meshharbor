import { Collapsible, CollapsibleContent, CollapsibleTrigger } from '@/components/ui/collapsible';
import { ListChecksIcon, SidebarSimpleIcon, StackIcon } from '@phosphor-icons/react';
import { useEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import { useDefaultLayout } from 'react-resizable-panels';
import { Button } from '@/components/ui/button';
import { ResizableHandle, ResizablePanel, ResizablePanelGroup } from '@/components/ui/resizable';
import { Sheet, SheetContent, SheetHeader, SheetTitle } from '@/components/ui/sheet';
import { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from '@/components/ui/tooltip';
import {
  decodeSidebarPanel,
  DESKTOP_SIDEBAR_KEY,
  toggleSidebarPanel,
  WORKBENCH_NARROW_QUERY,
  type SidebarPanel,
} from './workbench-state';

const sections = [
  { key: 'assets', label: 'Assets', icon: StackIcon },
  { key: 'jobs', label: 'Jobs', icon: ListChecksIcon },
] as const;

const panelStorage = {
  getItem(key: string) {
    try { return window.localStorage.getItem(key); }
    catch { return null; }
  },
  setItem(key: string, value: string) {
    try { window.localStorage.setItem(key, value); }
    catch { return; }
  },
};

export function useWorkbenchSidebar() {
  const [desktopPanel, setDesktopPanel] = useState<SidebarPanel | null>(() => {
    try { return decodeSidebarPanel(window.localStorage.getItem(DESKTOP_SIDEBAR_KEY)); }
    catch { return decodeSidebarPanel(null); }
  });
  const [mobilePanel, setMobilePanel] = useState<SidebarPanel | null>(null);
  const [narrow, setNarrow] = useState(() => window.matchMedia(WORKBENCH_NARROW_QUERY).matches);
  useEffect(() => {
    const media = window.matchMedia(WORKBENCH_NARROW_QUERY);
    const update = () => {
      setNarrow(media.matches);
      if (!media.matches) setMobilePanel(null);
    };
    update();
    media.addEventListener('change', update);
    return () => media.removeEventListener('change', update);
  }, []);
  function setPanel(next: SidebarPanel | null) {
    if (narrow) return setMobilePanel(next);
    setDesktopPanel(next);
    try {
      window.localStorage.setItem(DESKTOP_SIDEBAR_KEY, next ?? 'closed');
    } catch { /* Workbench remains usable if storage is disabled. */ }
  }
  return { narrow, panel: narrow ? mobilePanel : desktopPanel, setPanel };
}

export function WorkbenchLayout({
  state, assets, jobs, tools, inspector, children,
}: {
  state: ReturnType<typeof useWorkbenchSidebar>;
  assets: ReactNode;
  jobs: ReactNode;
  tools: ReactNode;
  inspector: ReactNode;
  children: ReactNode;
}) {
  const { panel, narrow, setPanel } = state;
  const rail = useRef<HTMLDivElement>(null);
  const lastTrigger = useRef<HTMLButtonElement | null>(null);
  const title = panel === 'jobs' ? 'Jobs' : 'Assets';
  const desktopDockOpen = !narrow && panel !== null;
  const inspectorOpen = Boolean(inspector);
  const panelIds = useMemo(() => [
    ...(inspectorOpen ? ['inspector'] : []),
    'viewport',
    ...(desktopDockOpen ? ['library'] : []),
  ], [desktopDockOpen, inspectorOpen]);
  const { defaultLayout, onLayoutChanged } = useDefaultLayout({
    id: 'meshharbor.studio.panels.v2',
    panelIds,
    storage: panelStorage,
    onlySaveAfterUserInteractions: true,
  });

  const restoreFocus = () => {
    const trigger = lastTrigger.current;
    if (trigger?.isConnected) trigger.focus();
    else rail.current?.querySelector<HTMLButtonElement>(`button[data-panel="${panel ?? 'assets'}"]`)?.focus();
  };

  function onCollapse() {
    setPanel(null);
    if (!narrow) requestAnimationFrame(restoreFocus);
  }

  const body = panel === 'assets' ? assets : panel === 'jobs' ? jobs : null;
  return (
    <Collapsible asChild open={desktopDockOpen} onOpenChange={open => {
      if (!open && !narrow) onCollapse();
    }}>
      <div className={narrow
        ? 'grid min-h-0 min-w-0 flex-1 grid-cols-[minmax(0,1fr)_auto] grid-rows-[minmax(0,1fr)_auto]'
        : 'grid min-h-0 min-w-0 flex-1 grid-cols-[auto_minmax(0,1fr)_auto]'}>
        <div className={narrow ? 'col-start-1 row-start-2 border-t bg-sidebar' : 'border-r bg-sidebar'}>{tools}</div>
        <ResizablePanelGroup id="studio-panels" orientation="horizontal"
          className={narrow ? 'col-span-2 col-start-1 row-start-1 min-h-0 min-w-0' : 'min-h-0 min-w-0'}
          defaultLayout={defaultLayout} onLayoutChanged={onLayoutChanged}>
          {inspectorOpen && <>
            <ResizablePanel id="inspector" defaultSize={288} minSize={240} maxSize={416} groupResizeBehavior="preserve-pixel-size" className="min-h-0 bg-sidebar text-sidebar-foreground">
              {inspector}
            </ResizablePanel>
            <ResizableHandle withHandle aria-label="Resize settings panel" />
          </>}
          <ResizablePanel id="viewport" minSize="28%" className="flex min-h-0 min-w-0 flex-col">
            {children}
          </ResizablePanel>
          {desktopDockOpen && <>
            <ResizableHandle withHandle aria-label="Resize library panel" />
            <ResizablePanel id="library" defaultSize={248} minSize={224} maxSize={416} groupResizeBehavior="preserve-pixel-size" className="min-h-0 bg-sidebar text-sidebar-foreground">
              <CollapsibleContent id="workbench-sidebar-dock" aria-label={title} className="h-full min-w-0 overflow-auto overscroll-contain p-4"
                onKeyDown={event => {
                  if (event.key === 'Escape') {
                    event.stopPropagation();
                    onCollapse();
                  }
                }}>
                <div className="mb-4 flex items-center justify-between gap-2">
                  <h2 className="font-heading text-sm font-medium">{title}</h2>
                  <CollapsibleTrigger asChild>
                    <Button variant="ghost" size="icon-sm" aria-label="Collapse sidebar" title="Collapse sidebar">
                      <SidebarSimpleIcon aria-hidden="true" />
                    </Button>
                  </CollapsibleTrigger>
                </div>
                {body}
              </CollapsibleContent>
            </ResizablePanel>
          </>}
        </ResizablePanelGroup>
        <div className={narrow ? 'col-start-2 row-start-2 border-t border-l bg-sidebar' : 'border-l bg-sidebar'}>
          <div className={narrow ? 'flex gap-1 p-2' : 'flex flex-col gap-2 p-2'} role="group" aria-label="Asset inventory" ref={rail}>
            <TooltipProvider delayDuration={180} disableHoverableContent>
              {sections.map(({ key, label, icon: Icon }) => (
                <Tooltip key={key}>
                  <TooltipTrigger asChild>
                    <Button
                      type="button"
                      variant={panel === key ? 'secondary' : 'ghost'}
                      size="icon"
                      aria-label={label}
                      data-panel={key}
                      aria-pressed={panel === key}
                      aria-expanded={panel === key}
                      aria-controls={panel === key ? (narrow ? 'workbench-sidebar-drawer' : 'workbench-sidebar-dock') : undefined}
                      aria-haspopup={narrow ? 'dialog' : undefined}
                      onClick={(event) => {
                        lastTrigger.current = event.currentTarget;
                        setPanel(toggleSidebarPanel(panel, key));
                      }}
                    ><Icon aria-hidden="true" /></Button>
                  </TooltipTrigger>
                  <TooltipContent side={narrow ? 'top' : 'left'} sideOffset={8}>{label}</TooltipContent>
                </Tooltip>
              ))}
            </TooltipProvider>
          </div>
        </div>
        {narrow && <Sheet open={panel !== null} onOpenChange={open => { if (!open) onCollapse(); }}>
          <SheetContent id="workbench-sidebar-drawer" side="right" aria-describedby={undefined} className="overflow-auto overscroll-contain max-sm:w-full"
            onCloseAutoFocus={event => { event.preventDefault(); restoreFocus(); }}>
            <SheetHeader><SheetTitle>{title}</SheetTitle></SheetHeader>
            <div className="min-w-0 px-4 pb-4">{body}</div>
          </SheetContent>
        </Sheet>}
      </div>
    </Collapsible>
  );
}

import { ListChecksIcon, SidebarSimpleIcon, SquaresFourIcon } from '@phosphor-icons/react';
import { useEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import { useDefaultLayout } from 'react-resizable-panels';
import { Button } from '@/components/ui/button';
import { ResizableHandle, ResizablePanel, ResizablePanelGroup } from '@/components/ui/resizable';
import { Sheet, SheetContent, SheetHeader, SheetTitle } from '@/components/ui/sheet';
import {
  decodeSidebarPanel,
  DESKTOP_SIDEBAR_KEY,
  WORKBENCH_NARROW_QUERY,
  type SidebarPanel,
} from './workbench-state';

const sections = [
  { key: 'assets', label: 'Assets', icon: SquaresFourIcon },
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
  const triggerRef = useRef<HTMLButtonElement>(null);
  const lastPanel = useRef<SidebarPanel>(desktopPanel ?? 'assets');
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
    if (next) lastPanel.current = next;
    if (narrow) return setMobilePanel(next);
    setDesktopPanel(next);
    try {
      window.localStorage.setItem(DESKTOP_SIDEBAR_KEY, next ?? 'closed');
    } catch { /* Workbench remains usable if storage is disabled. */ }
  }
  const panel = narrow ? mobilePanel : desktopPanel;
  return {
    narrow, panel, setPanel, triggerRef,
    toggleInventory: () => setPanel(panel ? null : lastPanel.current),
  };
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
  const { panel, narrow, setPanel, triggerRef } = state;
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

  const restoreFocus = () => triggerRef.current?.focus();

  function onCollapse() {
    setPanel(null);
    if (!narrow) requestAnimationFrame(restoreFocus);
  }

  const views = <div className="grid grid-cols-2 gap-1 rounded-lg bg-muted p-1" role="group" aria-label="Inventory views">
    {sections.map(({ key, label, icon: Icon }) => (
      <Button key={key} type="button" variant={panel === key ? 'outline' : 'ghost'} size="sm" aria-pressed={panel === key}
        onClick={() => setPanel(key)}><Icon aria-hidden="true" />{label}</Button>
    ))}
  </div>;
  const body = <div className="min-h-0 min-w-0 flex-1 overflow-x-hidden overflow-y-auto overscroll-contain">
    {panel === 'assets' ? assets : panel === 'jobs' ? jobs : null}
  </div>;

  return (
      <div className={narrow
        ? 'grid min-h-0 min-w-0 flex-1 grid-cols-1 grid-rows-[minmax(0,1fr)_auto]'
        : 'grid min-h-0 min-w-0 flex-1 grid-cols-[auto_minmax(0,1fr)]'}>
        <div className={narrow ? 'row-start-2 border-t bg-sidebar' : 'border-r bg-sidebar'}>{tools}</div>
        <ResizablePanelGroup id="studio-panels" orientation="horizontal"
          className={narrow ? 'row-start-1 min-h-0 min-w-0' : 'min-h-0 min-w-0'}
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
            <ResizablePanel id="library" defaultSize={304} minSize={224} maxSize={480} groupResizeBehavior="preserve-pixel-size" className="min-h-0 bg-sidebar text-sidebar-foreground">
              <aside id="workbench-sidebar-dock" aria-labelledby="workbench-inventory-title" className="flex h-full min-w-0 flex-col"
                onKeyDown={event => {
                  if (event.key === 'Escape') {
                    event.stopPropagation();
                    onCollapse();
                  }
                }}>
                <div className="grid shrink-0 gap-2 border-b px-3 pt-2 pb-3">
                  <div className="flex min-h-6 items-center justify-between gap-2">
                    <h2 id="workbench-inventory-title" className="font-heading text-sm font-medium">Inventory</h2>
                    <Button variant="ghost" size="icon-sm" aria-label="Collapse sidebar" title="Collapse sidebar" onClick={onCollapse}>
                      <SidebarSimpleIcon aria-hidden="true" />
                    </Button>
                  </div>
                  {views}
                </div>
                {body}
              </aside>
            </ResizablePanel>
          </>}
        </ResizablePanelGroup>
        {narrow && <Sheet open={panel !== null} onOpenChange={open => { if (!open) onCollapse(); }}>
          <SheetContent id="workbench-sidebar-drawer" side="right" aria-describedby={undefined} className="min-h-0 max-sm:w-full"
            onCloseAutoFocus={event => { event.preventDefault(); restoreFocus(); }}>
            <SheetHeader className="shrink-0 gap-3 border-b px-3 pt-4 pb-3"><SheetTitle>Inventory</SheetTitle>{views}</SheetHeader>
            {body}
          </SheetContent>
        </Sheet>}
      </div>
  );
}

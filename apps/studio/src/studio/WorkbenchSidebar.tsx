import { SidebarSimpleIcon } from '@phosphor-icons/react';
import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import { useDefaultLayout } from 'react-resizable-panels';
import { Button } from '@/components/ui/button';
import { ResizableHandle, ResizablePanel, ResizablePanelGroup } from '@/components/ui/resizable';
import { Sheet, SheetContent, SheetHeader, SheetTitle } from '@/components/ui/sheet';
import { Sidebar, SidebarContent, SidebarHeader, SidebarProvider } from '@/components/ui/sidebar';
import {
  decodeInventoryOpen,
  DESKTOP_SIDEBAR_KEY,
  WORKBENCH_NARROW_QUERY,
} from './workbench-state';

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

export type WorkbenchPanel = 'inventory' | 'jobs';

export function useWorkbenchSidebar(initialPanel: WorkbenchPanel = 'inventory') {
  const [desktopOpen, setDesktopOpen] = useState(() => {
    if (initialPanel === 'jobs') return true;
    try { return decodeInventoryOpen(window.localStorage.getItem(DESKTOP_SIDEBAR_KEY)); }
    catch { return true; }
  });
  const [mobileOpen, setMobileOpen] = useState(() => initialPanel === 'jobs' && window.matchMedia(WORKBENCH_NARROW_QUERY).matches);
  const [panel, setPanel] = useState(initialPanel);
  const inventoryTriggerRef = useRef<HTMLButtonElement>(null);
  const jobsTriggerRef = useRef<HTMLButtonElement>(null);
  const [narrow, setNarrow] = useState(() => window.matchMedia(WORKBENCH_NARROW_QUERY).matches);
  useEffect(() => {
    const media = window.matchMedia(WORKBENCH_NARROW_QUERY);
    const update = () => {
      setNarrow(media.matches);
      if (!media.matches) setMobileOpen(false);
    };
    media.addEventListener('change', update);
    return () => media.removeEventListener('change', update);
  }, []);
  const setOpen = useCallback((next: boolean) => {
    if (narrow) return setMobileOpen(next);
    setDesktopOpen(next);
    try {
      window.localStorage.setItem(DESKTOP_SIDEBAR_KEY, next ? 'assets' : 'closed');
    } catch { /* Workbench remains usable if storage is disabled. */ }
  }, [narrow]);
  const open = narrow ? mobileOpen : desktopOpen;
  const showPanel = useCallback((next: WorkbenchPanel) => {
    setPanel(next);
    setOpen(true);
  }, [setOpen]);
  return {
    narrow, open, panel, setOpen, showPanel, inventoryTriggerRef, jobsTriggerRef,
    triggerRef: panel === 'inventory' ? inventoryTriggerRef : jobsTriggerRef,
    togglePanel: (next: WorkbenchPanel) => open && panel === next ? setOpen(false) : showPanel(next),
  };
}

export function WorkbenchLayout({
  state, content, tools, inspector, children,
}: {
  state: ReturnType<typeof useWorkbenchSidebar>;
  content: ReactNode;
  tools: ReactNode;
  inspector: ReactNode;
  children: ReactNode;
}) {
  const { open, narrow, panel, setOpen, triggerRef } = state;
  const title = panel === 'inventory' ? 'Inventory' : 'Jobs';
  const desktopDockOpen = !narrow && open;
  const panelIds = useMemo(() => [
    'inspector',
    'viewport',
    ...(desktopDockOpen ? ['library'] : []),
  ], [desktopDockOpen]);
  const { defaultLayout, onLayoutChanged } = useDefaultLayout({
    id: narrow ? 'meshharbor.studio.stacked.v1' : 'meshharbor.studio.panels.v2',
    panelIds,
    storage: panelStorage,
    onlySaveAfterUserInteractions: true,
  });

  const restoreFocus = () => triggerRef.current?.focus();

  function onCollapse() {
    setOpen(false);
    if (!narrow) requestAnimationFrame(restoreFocus);
  }

  const body = <SidebarContent className="min-w-0 overflow-x-hidden overscroll-contain">
    {content}
  </SidebarContent>;

  return (
    <SidebarProvider className="contents" keyboardShortcut={false}>
      <div className="grid min-h-0 min-w-0 flex-1 grid-cols-[auto_minmax(0,1fr)]">
        <Sidebar collapsible="none" className="w-12 border-r border-sidebar-border">
          {tools}
        </Sidebar>
        <ResizablePanelGroup key={narrow ? 'stacked' : 'docked'} id="studio-panels" orientation={narrow ? 'vertical' : 'horizontal'}
          className="min-h-0 min-w-0"
          defaultLayout={defaultLayout} onLayoutChanged={onLayoutChanged}>
            <ResizablePanel id="inspector" defaultSize={narrow ? '46%' : 284} minSize={narrow ? '32%' : 224}
              maxSize={narrow ? '64%' : 416} groupResizeBehavior="preserve-pixel-size" className="min-h-0 bg-sidebar text-sidebar-foreground">
              <Sidebar collapsible="none" className="w-full">{inspector}</Sidebar>
            </ResizablePanel>
            <ResizableHandle withHandle aria-label="Resize settings panel" />
          <ResizablePanel id="viewport" minSize="28%" className="flex min-h-0 min-w-0 flex-col">
            {children}
          </ResizablePanel>
          {desktopDockOpen && <>
            <ResizableHandle withHandle aria-label={panel === 'jobs' ? 'Resize Jobs panel' : 'Resize library panel'} />
            <ResizablePanel id="library" defaultSize={304} minSize={224} maxSize={480} groupResizeBehavior="preserve-pixel-size" className="min-h-0 bg-sidebar text-sidebar-foreground">
              <Sidebar collapsible="none" side="right" className="w-full">
                <aside id="workbench-sidebar-dock" aria-labelledby="workbench-sidebar-title" className="flex h-full min-h-0 min-w-0 flex-col"
                  onKeyDown={event => {
                    if (event.key === 'Escape') {
                      event.stopPropagation();
                      onCollapse();
                    }
                  }}>
                  <SidebarHeader className="min-h-6 shrink-0 flex-row items-center justify-between border-b px-3 py-2">
                    <h2 id="workbench-sidebar-title" tabIndex={-1} className="font-heading text-sm font-medium outline-none">{title}</h2>
                    <Button variant="ghost" size="icon-sm" aria-label="Collapse sidebar" title="Collapse sidebar" onClick={onCollapse}>
                      <SidebarSimpleIcon aria-hidden="true" />
                    </Button>
                  </SidebarHeader>
                  {body}
                </aside>
              </Sidebar>
            </ResizablePanel>
          </>}
        </ResizablePanelGroup>
        {narrow && <Sheet open={open} onOpenChange={next => { if (!next) onCollapse(); }}>
          <SheetContent id="workbench-sidebar-drawer" side="right" aria-describedby={undefined} className="min-h-0 max-sm:w-full"
            onCloseAutoFocus={event => { event.preventDefault(); restoreFocus(); }}>
            <SheetHeader className="shrink-0 border-b px-3 py-4"><SheetTitle tabIndex={-1}>{title}</SheetTitle></SheetHeader>
            {body}
          </SheetContent>
        </Sheet>}
      </div>
    </SidebarProvider>
  );
}

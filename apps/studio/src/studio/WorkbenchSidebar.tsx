import { SidebarSimpleIcon } from '@phosphor-icons/react';
import { useEffect, useMemo, useRef, useState, type ReactNode } from 'react';
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

export function useWorkbenchSidebar() {
  const [desktopOpen, setDesktopOpen] = useState(() => {
    try { return decodeInventoryOpen(window.localStorage.getItem(DESKTOP_SIDEBAR_KEY)); }
    catch { return true; }
  });
  const [mobileOpen, setMobileOpen] = useState(false);
  const triggerRef = useRef<HTMLButtonElement>(null);
  const [narrow, setNarrow] = useState(() => window.matchMedia(WORKBENCH_NARROW_QUERY).matches);
  useEffect(() => {
    const media = window.matchMedia(WORKBENCH_NARROW_QUERY);
    const update = () => {
      setNarrow(media.matches);
      if (!media.matches) setMobileOpen(false);
    };
    update();
    media.addEventListener('change', update);
    return () => media.removeEventListener('change', update);
  }, []);
  function setOpen(next: boolean) {
    if (narrow) return setMobileOpen(next);
    setDesktopOpen(next);
    try {
      window.localStorage.setItem(DESKTOP_SIDEBAR_KEY, next ? 'assets' : 'closed');
    } catch { /* Workbench remains usable if storage is disabled. */ }
  }
  const open = narrow ? mobileOpen : desktopOpen;
  return {
    narrow, open, setOpen, triggerRef,
    toggleInventory: () => setOpen(!open),
  };
}

export function WorkbenchLayout({
  state, assets, tools, inspector, children,
}: {
  state: ReturnType<typeof useWorkbenchSidebar>;
  assets: ReactNode;
  tools: ReactNode;
  inspector: ReactNode;
  children: ReactNode;
}) {
  const { open, narrow, setOpen, triggerRef } = state;
  const desktopDockOpen = !narrow && open;
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
    setOpen(false);
    if (!narrow) requestAnimationFrame(restoreFocus);
  }

  const body = <SidebarContent className="min-w-0 overflow-x-hidden overscroll-contain">
    {assets}
  </SidebarContent>;

  return (
    <SidebarProvider className="contents" keyboardShortcut={false}>
      <div className={narrow
        ? 'grid min-h-0 min-w-0 flex-1 grid-cols-1 grid-rows-[minmax(0,1fr)_auto]'
        : 'grid min-h-0 min-w-0 flex-1 grid-cols-[auto_minmax(0,1fr)]'}>
        <Sidebar collapsible="none" className={narrow ? 'row-start-2 h-auto w-full border-t' : 'w-18 border-r border-sidebar-border'}>
          {tools}
        </Sidebar>
        <ResizablePanelGroup id="studio-panels" orientation="horizontal"
          className={narrow ? 'row-start-1 min-h-0 min-w-0' : 'min-h-0 min-w-0'}
          defaultLayout={defaultLayout} onLayoutChanged={onLayoutChanged}>
          {inspectorOpen && <>
            <ResizablePanel id="inspector" defaultSize={288} minSize={240} maxSize={416} groupResizeBehavior="preserve-pixel-size" className="min-h-0 bg-sidebar text-sidebar-foreground">
              <Sidebar collapsible="none" className="w-full">{inspector}</Sidebar>
            </ResizablePanel>
            <ResizableHandle withHandle aria-label="Resize settings panel" />
          </>}
          <ResizablePanel id="viewport" minSize="28%" className="flex min-h-0 min-w-0 flex-col">
            {children}
          </ResizablePanel>
          {desktopDockOpen && <>
            <ResizableHandle withHandle aria-label="Resize library panel" />
            <ResizablePanel id="library" defaultSize={304} minSize={224} maxSize={480} groupResizeBehavior="preserve-pixel-size" className="min-h-0 bg-sidebar text-sidebar-foreground">
              <Sidebar collapsible="none" side="right" className="w-full">
                <aside id="workbench-sidebar-dock" aria-labelledby="workbench-inventory-title" className="flex h-full min-h-0 min-w-0 flex-col"
                  onKeyDown={event => {
                    if (event.key === 'Escape') {
                      event.stopPropagation();
                      onCollapse();
                    }
                  }}>
                  <SidebarHeader className="min-h-6 shrink-0 flex-row items-center justify-between border-b px-3 py-2">
                    <h2 id="workbench-inventory-title" className="font-heading text-sm font-medium">Inventory</h2>
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
            <SheetHeader className="shrink-0 border-b px-3 py-4"><SheetTitle>Inventory</SheetTitle></SheetHeader>
            {body}
          </SheetContent>
        </Sheet>}
      </div>
    </SidebarProvider>
  );
}

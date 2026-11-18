import { Collapsible, CollapsibleContent, CollapsibleTrigger } from '../ui/Collapsible';
import { ListChecksIcon, SidebarSimpleIcon, StackIcon } from '@phosphor-icons/react';
import { useEffect, useRef, useState, type ReactNode } from 'react';
import { Button } from '../ui/Button';
import { Sheet, SheetClose } from '../ui/Sheet';
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

export function WorkbenchSidebar({
  state, assets, jobs, portal,
}: {
  state: ReturnType<typeof useWorkbenchSidebar>;
  assets: ReactNode;
  jobs: ReactNode;
  portal: HTMLElement | null;
}) {
  const { panel, narrow, setPanel } = state;
  const rail = useRef<HTMLDivElement>(null);
  const lastTrigger = useRef<HTMLButtonElement | null>(null);
  const title = panel === 'jobs' ? 'Jobs' : 'Assets';

  const restoreFocus = () => {
    const trigger = lastTrigger.current;
    if (trigger?.isConnected) trigger.focus();
    else rail.current?.querySelector<HTMLButtonElement>(`button[data-panel="${panel ?? 'assets'}"]`)?.focus();
  };

  function onCollapse() {
    setPanel(null);
    if (!narrow) requestAnimationFrame(restoreFocus);
  }

  const body = (
    <>
      <div className="workbench-dock-heading">
        <h2>{title}</h2>
        {narrow ? (
          <SheetClose asChild><Button variant="ghost" size="icon-sm" className="workbench-close" aria-label="Close sidebar" title="Close sidebar">
            <SidebarSimpleIcon aria-hidden="true" />
          </Button></SheetClose>
        ) : (
          <CollapsibleTrigger asChild><Button variant="ghost" size="icon-sm" className="workbench-close" aria-controls="workbench-sidebar-dock" aria-label="Collapse sidebar" title="Collapse sidebar">
            <SidebarSimpleIcon aria-hidden="true" />
          </Button></CollapsibleTrigger>
        )}
      </div>
      {panel === 'assets' ? assets : panel === 'jobs' ? jobs : null}
    </>
  );
  return (
    <Collapsible asChild open={!narrow && panel !== null} onOpenChange={open => {
      if (!open && !narrow) onCollapse();
    }}>
      <div className="workbench-sidebar" data-slot="sidebar">
        <div className="workbench-rail" role="group" aria-label="Studio sidebar" ref={rail}>
          {sections.map(({key,label,icon:Icon}) => (
            <Button
              type="button"
              variant="ghost"
              size="icon"
              className="workbench-rail-button"
              aria-label={label}
              data-panel={key}
              aria-pressed={panel === key}
              aria-expanded={panel === key}
              aria-controls={panel === key ? (narrow ? 'workbench-sidebar-drawer' : 'workbench-sidebar-dock') : undefined}
              aria-haspopup={narrow ? 'dialog' : undefined}
              title={label}
              onClick={(event) => {
                lastTrigger.current = event.currentTarget;
                setPanel(toggleSidebarPanel(panel, key));
              }}
              key={key}
            ><Icon aria-hidden="true" /><span>{label}</span></Button>
          ))}
        </div>
        {!narrow && <CollapsibleContent className="workbench-dock" id="workbench-sidebar-dock" aria-label={title} onKeyDown={event => {
          if (event.key === 'Escape') {
            event.stopPropagation();
            onCollapse();
          }
        }}>{body}</CollapsibleContent>}
        {narrow && <Sheet open={panel !== null} onOpenChange={open => { if (!open) onCollapse(); }}
          title={title} portal={portal} onCloseAutoFocus={event => { event.preventDefault(); restoreFocus(); }}>
          {body}
        </Sheet>}
      </div>
    </Collapsible>
  );
}

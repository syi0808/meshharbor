import { QuestionIcon } from '@phosphor-icons/react';
import { useState, type ReactNode } from 'react';
import { Button } from '@/components/ui/button';
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip';

export function HelpTip({ label, children }: { label: string; children: ReactNode }) {
  const [open, setOpen] = useState(false);
  return <Tooltip open={open} onOpenChange={setOpen}>
    <TooltipTrigger asChild>
      <Button type="button" variant="ghost" size="icon-xs" className="text-muted-foreground"
        aria-label={label} onClick={event => { event.preventDefault(); setOpen(true); }}>
        <QuestionIcon aria-hidden="true" className="size-3.5" />
      </Button>
    </TooltipTrigger>
    <TooltipContent sideOffset={4} className="max-w-[min(20rem,calc(100vw-1rem))] text-left wrap-anywhere">
      <span>{children}</span>
    </TooltipContent>
  </Tooltip>;
}

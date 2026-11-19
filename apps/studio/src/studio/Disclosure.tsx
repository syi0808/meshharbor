import { CaretRightIcon } from '@phosphor-icons/react';
import type { ReactNode } from 'react';
import { Button } from '@/components/ui/button';
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from '@/components/ui/collapsible';

export function Disclosure({ title, label, className, contentClassName, children }: {
  title: string;
  label?: string;
  className?: string;
  contentClassName?: string;
  children: ReactNode;
}) {
  return <Collapsible className={className} role={label ? 'group' : undefined} aria-label={label}>
    <CollapsibleTrigger asChild>
      <Button variant="ghost" size="sm" className="-ml-1 gap-1 px-1">
        <CaretRightIcon aria-hidden="true" className="size-3 transition-transform group-aria-expanded/button:rotate-90 motion-reduce:transition-none" />
        {title}
      </Button>
    </CollapsibleTrigger>
    <CollapsibleContent className={contentClassName}>{children}</CollapsibleContent>
  </Collapsible>;
}

import { HTMLAttributes, ReactNode } from "react";

import { cn } from "@/lib/utils";

interface PanelProps extends HTMLAttributes<HTMLDivElement> {
  title?: string;
  trailing?: ReactNode;
  bodyClassName?: string;
}

/**
 * Square paneled section used everywhere in the app. Title strip is rendered
 * with mono uppercase text in the header band, body is a thin-bordered area.
 *
 * Visually distinct from cc-switch's rounded glass-cards.
 */
export function Panel({
  title,
  trailing,
  className,
  bodyClassName,
  children,
  ...rest
}: PanelProps) {
  return (
    <section className={cn("panel", className)} {...rest}>
      {(title || trailing) && (
        <header className="panel-header">
          <span>{title}</span>
          {trailing && <div className="flex items-center gap-2">{trailing}</div>}
        </header>
      )}
      <div className={cn("p-4", bodyClassName)}>{children}</div>
    </section>
  );
}

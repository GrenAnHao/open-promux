import * as LabelPrimitive from "@radix-ui/react-label";
import { type ComponentPropsWithoutRef } from "react";

import { cn } from "@/lib/utils";

export function Label({
  className,
  ...props
}: ComponentPropsWithoutRef<typeof LabelPrimitive.Root>) {
  return (
    <LabelPrimitive.Root
      className={cn(
        "font-mono text-[11px] uppercase tracking-[0.18em] text-ink-400",
        className,
      )}
      {...props}
    />
  );
}
Label.displayName = "Label";

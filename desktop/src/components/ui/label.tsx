import * as LabelPrimitive from "@radix-ui/react-label";
import { ComponentPropsWithoutRef, forwardRef } from "react";

import { cn } from "@/lib/utils";

export const Label = forwardRef<
  React.ElementRef<typeof LabelPrimitive.Root>,
  ComponentPropsWithoutRef<typeof LabelPrimitive.Root>
>(({ className, ...props }, ref) => (
  <LabelPrimitive.Root
    ref={ref}
    className={cn(
      "font-mono text-[11px] uppercase tracking-[0.18em] text-ink-400",
      className,
    )}
    {...props}
  />
));
Label.displayName = "Label";

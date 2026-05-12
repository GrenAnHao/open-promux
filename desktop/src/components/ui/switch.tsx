import * as SwitchPrimitive from "@radix-ui/react-switch";
import { type ComponentPropsWithoutRef } from "react";

import { cn } from "@/lib/utils";

// Square slider - intentionally NOT the rounded "pill" used by most shadcn
// installs and definitely unlike the rounded glass switch in cc-switch.
export function Switch({
  className,
  ...props
}: ComponentPropsWithoutRef<typeof SwitchPrimitive.Root>) {
  return (
    <SwitchPrimitive.Root
      className={cn(
        "relative inline-flex h-5 w-9 shrink-0 cursor-pointer items-center border border-carbon-400 bg-carbon-700 transition-colors",
        "focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-mint-400/70 focus-visible:ring-offset-0",
        "data-[state=checked]:border-mint-400/70 data-[state=checked]:bg-mint-400/15",
        "disabled:cursor-not-allowed disabled:opacity-40",
        className,
      )}
      {...props}
    >
      <SwitchPrimitive.Thumb
        className={cn(
          "pointer-events-none block size-3 bg-ink-300 transition-transform",
          "data-[state=checked]:translate-x-4 data-[state=checked]:bg-mint-400",
          "data-[state=unchecked]:translate-x-1",
        )}
      />
    </SwitchPrimitive.Root>
  );
}
Switch.displayName = "Switch";

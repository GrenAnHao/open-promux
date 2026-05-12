import * as TabsPrimitive from "@radix-ui/react-tabs";
import { type ComponentPropsWithoutRef } from "react";

import { cn } from "@/lib/utils";

export const Tabs = TabsPrimitive.Root;

export function TabsList({
  className,
  ...props
}: ComponentPropsWithoutRef<typeof TabsPrimitive.List>) {
  return (
    <TabsPrimitive.List
      className={cn(
        "flex items-stretch border-b border-carbon-500 bg-carbon-900/40",
        className,
      )}
      {...props}
    />
  );
}
TabsList.displayName = "TabsList";

export function TabsTrigger({
  className,
  ...props
}: ComponentPropsWithoutRef<typeof TabsPrimitive.Trigger>) {
  return (
    <TabsPrimitive.Trigger
      className={cn(
        "relative inline-flex items-center gap-2 px-4 py-2 font-mono text-[12px] uppercase tracking-[0.18em] text-ink-500 transition-colors",
        "hover:text-ink-200",
        "focus-visible:outline-none focus-visible:bg-carbon-700/40",
        "data-[state=active]:text-mint-300",
        // Active indicator: 2px bottom line in mint instead of pill background.
        "after:pointer-events-none after:absolute after:inset-x-3 after:bottom-0 after:h-[2px] after:bg-transparent data-[state=active]:after:bg-mint-400",
        className,
      )}
      {...props}
    />
  );
}
TabsTrigger.displayName = "TabsTrigger";

export function TabsContent({
  className,
  ...props
}: ComponentPropsWithoutRef<typeof TabsPrimitive.Content>) {
  return (
    <TabsPrimitive.Content
      className={cn(
        "focus-visible:outline-none data-[state=inactive]:hidden",
        className,
      )}
      {...props}
    />
  );
}
TabsContent.displayName = "TabsContent";

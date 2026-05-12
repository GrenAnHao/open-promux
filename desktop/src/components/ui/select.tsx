import * as SelectPrimitive from "@radix-ui/react-select";
import { Check, ChevronDown } from "lucide-react";
import { type ComponentPropsWithoutRef, type ReactNode } from "react";

import { cn } from "@/lib/utils";

export const Select = SelectPrimitive.Root;
export const SelectValue = SelectPrimitive.Value;

export function SelectTrigger({
  className,
  children,
  ...props
}: ComponentPropsWithoutRef<typeof SelectPrimitive.Trigger>) {
  return (
    <SelectPrimitive.Trigger
      className={cn(
        "flex h-8 w-full items-center justify-between border border-carbon-500 bg-carbon-800/60 px-2.5",
        "font-mono text-[13px] text-ink-100",
        "hover:border-carbon-400 focus:border-mint-400/70 focus:outline-none",
        "data-[placeholder]:text-ink-700",
        "disabled:opacity-40",
        className,
      )}
      {...props}
    >
      {children}
      <SelectPrimitive.Icon asChild>
        <ChevronDown className="size-3.5 text-ink-500" />
      </SelectPrimitive.Icon>
    </SelectPrimitive.Trigger>
  );
}
SelectTrigger.displayName = "SelectTrigger";

export function SelectContent({
  className,
  children,
  position = "popper",
  ...props
}: ComponentPropsWithoutRef<typeof SelectPrimitive.Content>) {
  return (
    <SelectPrimitive.Portal>
      <SelectPrimitive.Content
        position={position}
        collisionPadding={10}
        className={cn(
          "relative z-50 max-h-[var(--radix-select-content-available-height)] min-w-[8rem] overflow-hidden border border-carbon-400 bg-carbon-700 shadow-glow",
          position === "popper" &&
            "data-[side=bottom]:translate-y-1 data-[side=top]:-translate-y-1",
          className,
        )}
        {...props}
      >
        <SelectPrimitive.Viewport
          className={cn(
            "max-h-[var(--radix-select-content-available-height)] overflow-y-auto p-1",
            position === "popper" &&
              "w-full min-w-[var(--radix-select-trigger-width)]",
          )}
        >
          {children}
        </SelectPrimitive.Viewport>
      </SelectPrimitive.Content>
    </SelectPrimitive.Portal>
  );
}
SelectContent.displayName = "SelectContent";

type SelectItemProps = ComponentPropsWithoutRef<typeof SelectPrimitive.Item> & {
  action?: ReactNode;
};

export function SelectItem({
  className,
  children,
  action,
  ...props
}: SelectItemProps) {
  return (
    <SelectPrimitive.Item
      className={cn(
        "relative flex cursor-pointer select-none items-center gap-2 px-2 py-1.5 pr-7 font-mono text-[12.5px] text-ink-100",
        "focus:bg-carbon-600 focus:text-ink-50 focus:outline-none",
        "data-[state=checked]:text-mint-300",
        action && "pr-14",
        className,
      )}
      {...props}
    >
      <SelectPrimitive.ItemText className="min-w-0 flex-1 truncate">
        {children}
      </SelectPrimitive.ItemText>
      {action ? (
        <span className="absolute right-1 flex size-5 items-center justify-center">
          {action}
        </span>
      ) : null}
      <span
        className={cn(
          "absolute flex size-3 items-center justify-center",
          action ? "right-7" : "right-2",
        )}
      >
        <SelectPrimitive.ItemIndicator>
          <Check className="size-3 text-mint-400" />
        </SelectPrimitive.ItemIndicator>
      </span>
    </SelectPrimitive.Item>
  );
}
SelectItem.displayName = "SelectItem";

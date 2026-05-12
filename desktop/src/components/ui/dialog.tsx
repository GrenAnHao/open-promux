import * as DialogPrimitive from "@radix-ui/react-dialog";
import { X } from "lucide-react";
import { type ComponentPropsWithoutRef } from "react";

import { cn } from "@/lib/utils";

export const Dialog = DialogPrimitive.Root;
export const DialogClose = DialogPrimitive.Close;

function DialogOverlay({
  className,
  ...props
}: ComponentPropsWithoutRef<typeof DialogPrimitive.Overlay>) {
  return (
    <DialogPrimitive.Overlay
      className={cn(
        "fixed inset-0 z-50 bg-carbon-950/70 backdrop-blur-[2px] data-[state=open]:animate-in data-[state=closed]:animate-out",
        className,
      )}
      {...props}
    />
  );
}
DialogOverlay.displayName = "DialogOverlay";

export function DialogContent({
  className,
  children,
  ...props
}: ComponentPropsWithoutRef<typeof DialogPrimitive.Content>) {
  return (
    <DialogPrimitive.Portal>
      <DialogOverlay />
      <DialogPrimitive.Content
        // Opt out of Radix's `aria-describedby` requirement unless the caller
        // explicitly provides one. Our dialogs use a single title + form body
        // without a separate description string.
        aria-describedby={undefined}
        className={cn(
          "fixed left-1/2 top-1/2 z-50 grid w-full max-w-2xl -translate-x-1/2 -translate-y-1/2 gap-0",
          "border border-carbon-400 bg-carbon-800 shadow-glow",
          className,
        )}
        {...props}
      >
        {children}
        <DialogPrimitive.Close
          aria-label="Close"
          className="absolute right-2 top-2 inline-flex size-7 items-center justify-center text-ink-400 hover:text-ink-100 focus:outline-none"
        >
          <X className="size-4" />
        </DialogPrimitive.Close>
      </DialogPrimitive.Content>
    </DialogPrimitive.Portal>
  );
}
DialogContent.displayName = "DialogContent";

export function DialogHeader({
  className,
  ...props
}: React.HTMLAttributes<HTMLDivElement>) {
  return (
    <div
      className={cn(
        "border-b border-carbon-500 bg-carbon-700/40 px-4 py-3 text-[11px] uppercase tracking-[0.18em] text-ink-400",
        className,
      )}
      {...props}
    />
  );
}

export function DialogTitle({
  className,
  ...props
}: ComponentPropsWithoutRef<typeof DialogPrimitive.Title>) {
  return (
    <DialogPrimitive.Title
      // Radix renders `<h2>`. We want it to inherit the DialogHeader's
      // mono uppercase typography instead of the browser's h2 defaults.
      className={cn(
        "m-0 font-[inherit] text-[inherit] tracking-[inherit]",
        className,
      )}
      {...props}
    />
  );
}
DialogTitle.displayName = "DialogTitle";

export function DialogFooter({
  className,
  ...props
}: React.HTMLAttributes<HTMLDivElement>) {
  return (
    <div
      className={cn(
        "flex items-center justify-end gap-2 border-t border-carbon-500 bg-carbon-700/30 px-4 py-3",
        className,
      )}
      {...props}
    />
  );
}

import { InputHTMLAttributes, forwardRef } from "react";

import { cn } from "@/lib/utils";

export const Input = forwardRef<
  HTMLInputElement,
  InputHTMLAttributes<HTMLInputElement>
>(({ className, type, ...props }, ref) => (
  <input
    ref={ref}
    type={type}
    className={cn(
      "h-8 w-full bg-carbon-800/60 border border-carbon-500 px-2.5 font-mono text-[13px] text-ink-100 placeholder:text-ink-700",
      "transition-colors hover:border-carbon-400 focus:border-mint-400/70 focus:outline-none focus:bg-carbon-800",
      "disabled:opacity-40",
      className,
    )}
    {...props}
  />
));
Input.displayName = "Input";

export const Textarea = forwardRef<
  HTMLTextAreaElement,
  React.TextareaHTMLAttributes<HTMLTextAreaElement>
>(({ className, ...props }, ref) => (
  <textarea
    ref={ref}
    className={cn(
      "w-full bg-carbon-800/60 border border-carbon-500 p-2.5 font-mono text-[12.5px] text-ink-100 placeholder:text-ink-700",
      "transition-colors hover:border-carbon-400 focus:border-mint-400/70 focus:outline-none focus:bg-carbon-800",
      "scrollbar-thin",
      className,
    )}
    {...props}
  />
));
Textarea.displayName = "Textarea";

import { Slot } from "@radix-ui/react-slot";
import { cva, type VariantProps } from "class-variance-authority";
import { ButtonHTMLAttributes, forwardRef } from "react";

import { cn } from "@/lib/utils";

const buttonVariants = cva(
  "inline-flex items-center justify-center gap-1.5 whitespace-nowrap font-mono text-[12px] uppercase tracking-[0.14em] transition-colors disabled:pointer-events-none disabled:opacity-40 focus-visible:outline-none",
  {
    variants: {
      variant: {
        // Primary action: hollow mint border, fills on hover.
        primary:
          "border border-mint-400/70 text-mint-300 hover:bg-mint-400/10 hover:text-mint-200 active:bg-mint-400/20",
        // Secondary: faint outline, ink text. Used for cancel/secondary CTA.
        ghost:
          "border border-carbon-400 text-ink-200 hover:border-ink-400 hover:text-ink-50",
        // Destructive: coral border, only used for delete/destroy.
        danger:
          "border border-coral-400/70 text-coral-400 hover:bg-coral-400/10 hover:text-coral-400/95",
        // Plain text-only, no chrome.
        link:
          "text-ink-300 hover:text-ink-50 underline-offset-4 hover:underline px-0",
      },
      size: {
        sm: "h-7 px-2.5",
        md: "h-8 px-3",
        lg: "h-10 px-4 text-[13px]",
        icon: "h-8 w-8 p-0",
      },
    },
    defaultVariants: {
      variant: "ghost",
      size: "md",
    },
  },
);

export interface ButtonProps
  extends ButtonHTMLAttributes<HTMLButtonElement>,
    VariantProps<typeof buttonVariants> {
  asChild?: boolean;
}

export const Button = forwardRef<HTMLButtonElement, ButtonProps>(
  ({ className, variant, size, asChild = false, ...props }, ref) => {
    const Comp = asChild ? Slot : "button";
    return (
      <Comp
        ref={ref}
        className={cn(buttonVariants({ variant, size }), className)}
        {...props}
      />
    );
  },
);
Button.displayName = "Button";

export { buttonVariants };

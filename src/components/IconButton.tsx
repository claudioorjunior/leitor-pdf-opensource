import type { ButtonHTMLAttributes } from "react";
import { cn } from "@/lib/utils";

export function IconButton({
  className,
  active,
  ...props
}: ButtonHTMLAttributes<HTMLButtonElement> & { active?: boolean }) {
  return (
    <button
      type="button"
      data-on={active ? "true" : undefined}
      className={cn("icon-btn", className)}
      {...props}
    />
  );
}

import { cn } from "@/lib/utils";
import type { ComponentProps } from "react";

export function Badge({
  className,
  tone = "muted",
  ...props
}: ComponentProps<"span"> & {
  tone?: "muted" | "ok" | "warn" | "bad" | "teal";
}) {
  const tones = {
    muted: "bg-ink/6 text-muted",
    ok: "bg-teal-soft text-teal",
    warn: "bg-[#f4e2d4] text-seal",
    bad: "bg-[#f4d6d4] text-danger",
    teal: "bg-teal text-white",
  };
  return (
    <span
      className={cn(
        "inline-flex items-center rounded-full px-2 py-0.5 text-[11px] font-medium tracking-wide",
        tones[tone],
        className,
      )}
      {...props}
    />
  );
}

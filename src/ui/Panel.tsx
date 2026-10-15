import { useId, type ReactNode } from "react";

interface PanelProps {
  title?: string;
  /** Decorative icon before the title. */
  icon?: ReactNode;
  children: ReactNode;
}

/** Surface container with a border and 12px radius, optionally titled. */
export function Panel({ title, icon, children }: PanelProps) {
  const headingId = useId();
  return (
    <section aria-labelledby={title ? headingId : undefined} className="rounded-panel border border-border bg-surface">
      {title ? (
        <h2 id={headingId} className="flex items-center gap-2 px-4 pt-4 pb-1 text-base font-semibold text-text">
          {icon ? <span className="text-text-muted">{icon}</span> : null}
          {title}
        </h2>
      ) : null}
      {children}
    </section>
  );
}

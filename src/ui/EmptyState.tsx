import type { ReactNode } from "react";

interface EmptyStateProps {
  icon: ReactNode;
  title: string;
  description: string;
}

/** Centered icon, title and one sentence for views with nothing to show. */
export function EmptyState({ icon, title, description }: EmptyStateProps) {
  return (
    <div className="flex flex-col items-center gap-3 px-6 text-center">
      <div className="text-text-muted">{icon}</div>
      <h2 className="text-lg font-semibold text-text">{title}</h2>
      <p className="max-w-sm text-base text-text-muted">{description}</p>
    </div>
  );
}

import { useRef, type KeyboardEvent } from "react";
import { cx } from "../lib/cx";

export interface SegmentOption<T extends string> {
  value: T;
  label: string;
}

interface SegmentedControlProps<T extends string> {
  value: T;
  options: ReadonlyArray<SegmentOption<T>>;
  onChange: (value: T) => void;
  /** Accessible name of the group. */
  label: string;
}

/** Radiogroup with arrow-key navigation; the selection indicator slides with transform only. */
export function SegmentedControl<T extends string>({ value, options, onChange, label }: SegmentedControlProps<T>) {
  const refs = useRef<Array<HTMLButtonElement | null>>([]);
  const count = options.length;
  const index = Math.max(
    0,
    options.findIndex((o) => o.value === value),
  );

  function move(to: number) {
    const next = options[(to + count) % count];
    if (!next) return;
    onChange(next.value);
    refs.current[(to + count) % count]?.focus();
  }

  function onKeyDown(event: KeyboardEvent<HTMLButtonElement>) {
    switch (event.key) {
      case "ArrowRight":
      case "ArrowDown":
        event.preventDefault();
        move(index + 1);
        break;
      case "ArrowLeft":
      case "ArrowUp":
        event.preventDefault();
        move(index - 1);
        break;
      case "Home":
        event.preventDefault();
        move(0);
        break;
      case "End":
        event.preventDefault();
        move(count - 1);
        break;
    }
  }

  return (
    <div
      role="radiogroup"
      aria-label={label}
      className="relative inline-grid auto-cols-fr grid-flow-col rounded-control border border-border bg-surface-2 p-0.5"
    >
      <span
        aria-hidden="true"
        className="t-slide pointer-events-none absolute top-0.5 bottom-0.5 left-0.5 rounded-[6px] bg-accent shadow-float"
        style={{
          width: `calc((100% - 4px) / ${count})`,
          transform: `translateX(${index * 100}%)`,
        }}
      />
      {options.map((option, i) => {
        const selected = i === index;
        return (
          <button
            key={option.value}
            ref={(el) => {
              refs.current[i] = el;
            }}
            type="button"
            role="radio"
            aria-checked={selected}
            tabIndex={selected ? 0 : -1}
            onClick={() => onChange(option.value)}
            onKeyDown={onKeyDown}
            className={cx(
              "t-colors relative z-10 h-7 rounded-[6px] px-3 text-sm font-medium whitespace-nowrap",
              selected ? "text-accent-fg" : "text-text-muted hover-fine:text-text",
            )}
          >
            {option.label}
          </button>
        );
      })}
    </div>
  );
}

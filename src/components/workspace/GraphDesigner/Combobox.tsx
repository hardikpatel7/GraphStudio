import { useEffect, useMemo, useRef, useState } from "react";
import { ChevronDown } from "lucide-react";

interface Option {
  value: string;
  /// Optional secondary label rendered to the right of the value
  /// (e.g. "table" / "view" for relation rows). Not searchable.
  hint?: string;
}

interface Props {
  value: string;
  onChange: (next: string) => void;
  /// Fires when the field loses focus AND the value has changed
  /// since last commit. Used by parent forms to persist on blur.
  onCommit?: (next: string) => void;
  options: Option[];
  placeholder?: string;
  /// Free-text mode lets the user enter values that aren't in
  /// `options` (typical for autocomplete inputs against an external
  /// universe — the user might paste in a table name we haven't
  /// discovered yet, or one that's about to be created).
  allowFreeText?: boolean;
  /// Extra inputs render below the input — surface validation
  /// messages, hints, etc.
  className?: string;
}

/// Editable autocomplete combobox.
///
/// - Typing filters `options` by case-insensitive substring match
/// - Matching characters are highlighted in the dropdown
/// - Arrow keys / Enter / Escape for keyboard nav
/// - Click outside closes the dropdown
/// - `onCommit` fires on blur with the final value (debounced via
///   `lastCommittedRef` so re-renders don't double-fire)
export function Combobox({
  value,
  onChange,
  onCommit,
  options,
  placeholder,
  allowFreeText = true,
  className,
}: Props) {
  const [open, setOpen] = useState(false);
  const [activeIndex, setActiveIndex] = useState(0);
  const containerRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLInputElement>(null);
  const lastCommittedRef = useRef<string>(value);

  // Filter the options against the current input.
  // Case-insensitive substring match. Cheap (handful of hundreds of
  // tables max) — no need for a fuzzy matcher here.
  const filtered = useMemo(() => {
    const q = value.trim().toLowerCase();
    if (!q) return options.slice(0, 50);
    return options
      .filter((o) => o.value.toLowerCase().includes(q))
      .slice(0, 50);
  }, [options, value]);

  // Reset active index whenever the filtered list changes so we
  // don't end up pointing at an invalid row.
  useEffect(() => {
    setActiveIndex(0);
  }, [value, options.length]);

  // Click-outside closes the dropdown and commits if the value
  // changed since last commit.
  useEffect(() => {
    if (!open) return;
    const onDocClick = (e: MouseEvent) => {
      if (!containerRef.current?.contains(e.target as Node)) {
        closeAndMaybeCommit();
      }
    };
    document.addEventListener("mousedown", onDocClick);
    return () => document.removeEventListener("mousedown", onDocClick);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, value]);

  const closeAndMaybeCommit = () => {
    setOpen(false);
    if (value !== lastCommittedRef.current) {
      lastCommittedRef.current = value;
      onCommit?.(value);
    }
  };

  const pick = (v: string) => {
    onChange(v);
    setOpen(false);
    if (v !== lastCommittedRef.current) {
      lastCommittedRef.current = v;
      onCommit?.(v);
    }
    inputRef.current?.blur();
  };

  return (
    <div ref={containerRef} className={"relative " + (className ?? "")}>
      <div className="relative">
        <input
          ref={inputRef}
          type="text"
          value={value}
          placeholder={placeholder}
          onChange={(e) => {
            onChange(e.target.value);
            setOpen(true);
          }}
          onFocus={() => setOpen(true)}
          onBlur={() => {
            // Defer so click-on-option fires before blur kills the
            // dropdown. The document-mousedown handler also covers
            // outside clicks; this is the keyboard path.
            setTimeout(() => {
              if (!containerRef.current?.contains(document.activeElement)) {
                closeAndMaybeCommit();
              }
            }, 100);
          }}
          onKeyDown={(e) => {
            if (e.key === "ArrowDown") {
              e.preventDefault();
              setOpen(true);
              setActiveIndex((i) => Math.min(filtered.length - 1, i + 1));
            } else if (e.key === "ArrowUp") {
              e.preventDefault();
              setActiveIndex((i) => Math.max(0, i - 1));
            } else if (e.key === "Enter") {
              if (open && filtered[activeIndex]) {
                e.preventDefault();
                pick(filtered[activeIndex].value);
              } else if (allowFreeText) {
                e.preventDefault();
                closeAndMaybeCommit();
              }
            } else if (e.key === "Escape") {
              setOpen(false);
            }
          }}
          spellCheck={false}
          className="w-full text-xs bg-gray-900 text-gray-200 rounded border border-gray-700 px-2 py-1.5 pr-7 font-mono focus:outline-none focus:border-blue-500"
        />
        <button
          type="button"
          onClick={() => {
            setOpen((v) => !v);
            inputRef.current?.focus();
          }}
          className="absolute right-1.5 top-1/2 -translate-y-1/2 text-gray-500 hover:text-gray-200 p-0.5"
          tabIndex={-1}
        >
          <ChevronDown size={12} />
        </button>
      </div>

      {open && filtered.length > 0 && (
        <ul
          className="absolute left-0 right-0 mt-1 max-h-64 overflow-auto bg-gray-900 border border-gray-700 rounded shadow-lg z-20"
          // Mousedown (not click) so we pick the option BEFORE the
          // input's blur fires and tears down the dropdown.
          onMouseDown={(e) => e.preventDefault()}
        >
          {filtered.map((opt, i) => (
            <li
              key={opt.value}
              onMouseEnter={() => setActiveIndex(i)}
              onClick={() => pick(opt.value)}
              className={
                "flex items-baseline gap-2 px-2 py-1 cursor-pointer text-xs font-mono " +
                (i === activeIndex ? "bg-blue-900/40 text-gray-100" : "text-gray-300 hover:bg-gray-800")
              }
            >
              <Highlight text={opt.value} query={value} />
              {opt.hint && (
                <span className="ml-auto text-[10px] text-gray-500 normal-case">
                  {opt.hint}
                </span>
              )}
            </li>
          ))}
        </ul>
      )}
      {open && filtered.length === 0 && value.trim() && (
        <div className="absolute left-0 right-0 mt-1 px-2 py-1.5 bg-gray-900 border border-gray-700 rounded text-[11px] text-gray-500 z-20">
          No matches. {allowFreeText && "Press Enter to use this name anyway."}
        </div>
      )}
    </div>
  );
}

/// Render `text` with substrings matching `query` wrapped in a
/// highlight span. Case-insensitive, first-match-wins for each
/// occurrence. Returns plain text when query is empty.
function Highlight({ text, query }: { text: string; query: string }) {
  const q = query.trim().toLowerCase();
  if (!q) return <span>{text}</span>;
  const lower = text.toLowerCase();
  const segments: { str: string; match: boolean }[] = [];
  let cursor = 0;
  while (cursor < text.length) {
    const idx = lower.indexOf(q, cursor);
    if (idx < 0) {
      segments.push({ str: text.slice(cursor), match: false });
      break;
    }
    if (idx > cursor) {
      segments.push({ str: text.slice(cursor, idx), match: false });
    }
    segments.push({ str: text.slice(idx, idx + q.length), match: true });
    cursor = idx + q.length;
  }
  return (
    <span>
      {segments.map((s, i) =>
        s.match ? (
          <mark
            key={i}
            className="bg-amber-400/30 text-amber-200 rounded-sm px-0.5"
          >
            {s.str}
          </mark>
        ) : (
          <span key={i}>{s.str}</span>
        ),
      )}
    </span>
  );
}

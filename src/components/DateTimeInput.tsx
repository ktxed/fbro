import { useEffect, useRef, useState } from "react";

/**
 * A locale-independent, always-24-hour date+time field.
 *
 * The native `<input type="datetime-local">` (and `type="time"`) render their
 * hour subfield in 12-hour form whenever the OS locale is an AM/PM locale,
 * which makes typing `0` clamp to `1`. WebKit (used by Tauri on macOS and
 * Linux) offers no attribute to override this. So we render our own segmented
 * control with explicit 24-hour semantics.
 *
 * `value`/`onChange` use the same string shape as datetime-local
 * (`YYYY-MM-DDTHH:mm`), so callers keep their existing epoch conversion. An
 * incomplete entry emits `""`.
 */

type SegKey = "month" | "day" | "year" | "hour" | "minute";

interface Segments {
  month: number | null;
  day: number | null;
  year: number | null;
  hour: number | null;
  minute: number | null;
}

const ORDER: SegKey[] = ["month", "day", "year", "hour", "minute"];
const MIN: Record<SegKey, number> = { month: 1, day: 1, year: 1, hour: 0, minute: 0 };
const MAX: Record<SegKey, number> = { month: 12, day: 31, year: 9999, hour: 23, minute: 59 };
const PLACEHOLDER: Record<SegKey, string> = {
  month: "mm",
  day: "dd",
  year: "yyyy",
  hour: "--",
  minute: "--",
};

const EMPTY: Segments = { month: null, day: null, year: null, hour: null, minute: null };

const pad = (n: number, len: number) => String(n).padStart(len, "0");

function parse(value: string): Segments {
  const m = value.match(/^(\d{4})-(\d{2})-(\d{2})T(\d{2}):(\d{2})/);
  if (!m) return { ...EMPTY };
  return {
    year: Number(m[1]),
    month: Number(m[2]),
    day: Number(m[3]),
    hour: Number(m[4]),
    minute: Number(m[5]),
  };
}

function build(s: Segments): string {
  if (
    s.month == null || s.day == null || s.year == null ||
    s.hour == null || s.minute == null
  ) {
    return "";
  }
  if (
    s.year < 1000 ||
    s.month < 1 || s.month > 12 ||
    s.day < 1 || s.day > 31 ||
    s.hour < 0 || s.hour > 23 ||
    s.minute < 0 || s.minute > 59
  ) {
    return "";
  }
  return `${pad(s.year, 4)}-${pad(s.month, 2)}-${pad(s.day, 2)}T${pad(s.hour, 2)}:${pad(s.minute, 2)}`;
}

interface DateTimeInputProps {
  value: string;
  onChange: (value: string) => void;
  /** Called when the user starts interacting (used to select the owning radio). */
  onInteract?: () => void;
  ariaLabel?: string;
  className?: string;
}

export function DateTimeInput({ value, onChange, onInteract, ariaLabel, className }: DateTimeInputProps) {
  const [segs, setSegs] = useState<Segments>(() => parse(value));
  const [focusedIdx, setFocusedIdx] = useState<number | null>(null);
  const [typed, setTyped] = useState("");
  const lastEmitted = useRef(value);
  const refs = useRef<(HTMLSpanElement | null)[]>([]);

  // Sync from an external value change (e.g. a reset), but ignore the echo of
  // our own emitted value so in-progress segment typing is not wiped.
  useEffect(() => {
    if (value !== lastEmitted.current) {
      setSegs(parse(value));
      setTyped("");
      lastEmitted.current = value;
    }
  }, [value]);

  const commit = (next: Segments, nextTyped: string) => {
    setSegs(next);
    setTyped(nextTyped);
    const out = build(next);
    lastEmitted.current = out;
    onChange(out);
  };

  const focusIdx = (i: number) => {
    if (i >= 0 && i < ORDER.length) refs.current[i]?.focus();
  };

  const onDigit = (key: SegKey, digit: string) => {
    onInteract?.();
    const idx = ORDER.indexOf(key);

    if (key === "year") {
      const acc = (typed + digit).slice(0, 4);
      const val = Number(acc);
      if (acc.length >= 4) {
        commit({ ...segs, year: val }, "");
        focusIdx(idx + 1);
      } else {
        commit({ ...segs, year: val }, acc);
      }
      return;
    }

    const max = MAX[key];
    if (typed === "") {
      const d = Number(digit);
      if (d * 10 > max) {
        // No valid second digit possible — commit and advance.
        commit({ ...segs, [key]: d }, "");
        focusIdx(idx + 1);
      } else {
        commit({ ...segs, [key]: d }, digit);
      }
    } else {
      const n = Math.min(Number(typed + digit), max);
      commit({ ...segs, [key]: n }, "");
      focusIdx(idx + 1);
    }
  };

  const step = (key: SegKey, dir: 1 | -1) => {
    onInteract?.();
    const cur = segs[key];
    let next: number;
    if (cur == null) {
      next = key === "year" ? new Date().getFullYear() : (dir > 0 ? MIN[key] : MAX[key]);
    } else if (key === "year") {
      next = Math.max(1, cur + dir);
    } else {
      const span = MAX[key] - MIN[key] + 1;
      next = ((cur - MIN[key] + dir) % span + span) % span + MIN[key];
    }
    commit({ ...segs, [key]: next }, "");
  };

  const clearSeg = (key: SegKey) => {
    onInteract?.();
    commit({ ...segs, [key]: null }, "");
  };

  const handleKeyDown = (e: React.KeyboardEvent) => {
    if (focusedIdx == null) return;
    const key = ORDER[focusedIdx];
    if (/^[0-9]$/.test(e.key)) {
      e.preventDefault();
      onDigit(key, e.key);
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      step(key, 1);
    } else if (e.key === "ArrowDown") {
      e.preventDefault();
      step(key, -1);
    } else if (e.key === "ArrowLeft") {
      e.preventDefault();
      focusIdx(focusedIdx - 1);
    } else if (e.key === "ArrowRight") {
      e.preventDefault();
      focusIdx(focusedIdx + 1);
    } else if (e.key === "Backspace" || e.key === "Delete") {
      e.preventDefault();
      clearSeg(key);
    } else if (e.key === "/" || e.key === ":" || e.key === "," || e.key === " ") {
      e.preventDefault();
      focusIdx(focusedIdx + 1);
    }
    // Tab is left to the browser: each segment is tabbable in order.
  };

  const display = (key: SegKey, isFocused: boolean): string => {
    if (isFocused && typed !== "") return typed;
    const v = segs[key];
    if (v == null) return PLACEHOLDER[key];
    return pad(v, key === "year" ? 4 : 2);
  };

  const renderSeg = (key: SegKey, idx: number) => {
    const isFocused = focusedIdx === idx;
    const isEmpty = segs[key] == null && !(isFocused && typed !== "");
    return (
      <span
        key={key}
        ref={(el) => { refs.current[idx] = el; }}
        role="spinbutton"
        tabIndex={0}
        aria-label={key}
        aria-valuenow={segs[key] ?? undefined}
        className={`dt-input__seg${isEmpty ? " dt-input__seg--empty" : ""}${isFocused ? " dt-input__seg--focused" : ""}`}
        onFocus={() => { setFocusedIdx(idx); setTyped(""); }}
        onBlur={() => { setFocusedIdx((cur) => (cur === idx ? null : cur)); setTyped(""); }}
      >
        {display(key, isFocused)}
      </span>
    );
  };

  return (
    <div
      className={`dt-input${className ? ` ${className}` : ""}`}
      role="group"
      aria-label={ariaLabel}
      onKeyDown={handleKeyDown}
    >
      {renderSeg("month", 0)}
      <span className="dt-input__sep">/</span>
      {renderSeg("day", 1)}
      <span className="dt-input__sep">/</span>
      {renderSeg("year", 2)}
      <span className="dt-input__sep">,&nbsp;</span>
      {renderSeg("hour", 3)}
      <span className="dt-input__sep">:</span>
      {renderSeg("minute", 4)}
    </div>
  );
}

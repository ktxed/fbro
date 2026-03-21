import { useEffect } from "react";

const ACTIONS = [
  { key: "F3",  label: "View",   icon: "👁️"  },
  { key: "F4",  label: "Edit",   icon: "✏️"  },
  { key: "F5",  label: "Copy",   icon: "📋"  },
  { key: "F6",  label: "Move",   icon: "✂️"  },
  { key: "F7",  label: "MkDir",  icon: "📁"  },
  { key: "F8",  label: "Delete", icon: "🗑️" },
  { key: "F9",  label: "Shell",  icon: "🖥️" },
  { key: "F10", label: "Quit",   icon: "🚪"  },
] as const;

export function Toolbar() {
  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      if (ACTIONS.some(a => a.key === e.key)) {
        e.preventDefault();
        // placeholder — operations not yet implemented
      }
    };
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, []);

  return (
    <div className="toolbar">
      {ACTIONS.map(({ key, label, icon }) => (
        <button key={key} className="toolbar__btn" tabIndex={-1} title={`${key} — ${label}`}>
          <span className="toolbar__key">{key}</span>
          <span className="toolbar__icon">{icon}</span>
          <span className="toolbar__label">{label}</span>
        </button>
      ))}
    </div>
  );
}

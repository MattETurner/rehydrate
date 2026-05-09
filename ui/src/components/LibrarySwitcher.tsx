import { Icon } from "./Icon";
import { Menu, type MenuItem } from "./Menu";
import type { RecentLibraryEntry } from "../types";

interface Props {
  currentPath: string | null;
  recents: RecentLibraryEntry[];
  onSwitch: (path: string) => void;
  onOpenAnother: () => void;
}

/* Toolbar chip showing the active library, with a dropdown listing
 * the user's other libraries. Designed for users who own more than
 * one reMarkable — each tablet pulls into its own library directory,
 * and switching libraries is a 2-click operation instead of a
 * relaunch with a different config.
 *
 * Stale entries (the directory has been moved or deleted) render
 * disabled so the user can see the history but not break things by
 * trying to open them. */
export function LibrarySwitcher({
  currentPath,
  recents,
  onSwitch,
  onOpenAnother,
}: Props) {
  const current =
    recents.find((r) => r.current) ??
    (currentPath ? { label: labelFor(currentPath), path: currentPath } : null);
  const label = current?.label ?? "Library";

  const items: MenuItem[] = [];
  for (const r of recents) {
    items.push({
      label: r.label || labelFor(r.path),
      icon: r.current ? <Icon name="check" /> : <Icon name="library" />,
      disabled: !r.available || r.current,
      onClick: () => onSwitch(r.path),
    });
  }
  items.push({
    label: "Open another library…",
    icon: <Icon name="library" />,
    separatorBefore: items.length > 0,
    onClick: () => onOpenAnother(),
  });

  return (
    <Menu
      align="left"
      trigger={
        <button
          className="library-switcher"
          title={currentPath ?? undefined}
          aria-label={`Switch library (current: ${label})`}
        >
          <span className="library-switcher__label">{label}</span>
          <Icon name="more" />
        </button>
      }
      items={items}
    />
  );
}

function labelFor(path: string): string {
  // Final path component, with trailing slashes stripped.
  const normalized = path.replace(/[/\\]+$/, "");
  const sep = Math.max(
    normalized.lastIndexOf("/"),
    normalized.lastIndexOf("\\"),
  );
  return sep >= 0 ? normalized.slice(sep + 1) : normalized;
}

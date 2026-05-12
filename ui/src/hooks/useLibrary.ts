// Library content state + the two refresh callbacks, extracted
// from App.tsx.
//
// The hook owns the four content lists (`documents`, `folders`,
// `archived`, `summary`), the open-state pair (`libraryOpen`,
// `libraryPath`), the recents list (`recentLibraries`), and the
// canonical refresh path.
//
// What stays in App.tsx: the imperative orchestrators
// (`openLibrary`, `switchToLibrary`, `openAnotherLibrary`) — those
// reach into selection state, expanded-folder state, and the
// confirm dialog, all of which would make the hook's surface
// unfocused. They consume the hook's setters directly.
//
// `refreshing` is a `useRef` boolean that short-circuits concurrent
// refreshes. The four IPC calls inside `refreshLibrary` are
// independent (`Promise.all`) but a second `refreshLibrary` while
// the first is mid-flight would just double the work and risk
// out-of-order setState. The guard sidesteps that without any
// debouncing or queueing complexity.

import { useCallback, useRef, useState } from "react";

import { formatError } from "../formatError";
import { ipc } from "../ipc";
import type {
  ArchivedDocument,
  DocumentSummary,
  FolderEntry,
  LibrarySummary,
  RecentLibraryEntry,
} from "../types";

export interface UseLibraryInjections {
  /// App-level error sink. Refresh failures route through it.
  setError: (msg: string | null) => void;
}

export interface UseLibraryResult {
  libraryOpen: boolean;
  libraryPath: string | null;
  recentLibraries: RecentLibraryEntry[];
  summary: LibrarySummary | null;
  documents: DocumentSummary[] | null;
  folders: FolderEntry[];
  archived: ArchivedDocument[];
  setLibraryOpen: React.Dispatch<React.SetStateAction<boolean>>;
  setLibraryPath: React.Dispatch<React.SetStateAction<string | null>>;
  setRecentLibraries: React.Dispatch<
    React.SetStateAction<RecentLibraryEntry[]>
  >;
  setSummary: React.Dispatch<React.SetStateAction<LibrarySummary | null>>;
  setDocuments: React.Dispatch<
    React.SetStateAction<DocumentSummary[] | null>
  >;
  setFolders: React.Dispatch<React.SetStateAction<FolderEntry[]>>;
  setArchived: React.Dispatch<React.SetStateAction<ArchivedDocument[]>>;
  /// Refetch all four content lists in parallel. Idempotent under
  /// concurrent calls — the second invocation returns immediately
  /// while the first is in flight.
  refreshLibrary: () => Promise<void>;
  /// Refetch only the recents list. Non-fatal on failure — the
  /// switcher just keeps showing whatever it had.
  refreshRecentLibraries: () => Promise<void>;
  /// Wipe in-memory content state. Used before a library switch so
  /// the user sees an obvious "loading" state instead of
  /// cross-library leakage during the switch round-trip.
  clearLibraryUi: () => void;
}

export function useLibrary(injections: UseLibraryInjections): UseLibraryResult {
  const { setError } = injections;
  const [libraryOpen, setLibraryOpen] = useState(false);
  const [libraryPath, setLibraryPath] = useState<string | null>(null);
  const [recentLibraries, setRecentLibraries] = useState<RecentLibraryEntry[]>(
    [],
  );
  const [summary, setSummary] = useState<LibrarySummary | null>(null);
  const [documents, setDocuments] = useState<DocumentSummary[] | null>(null);
  const [folders, setFolders] = useState<FolderEntry[]>([]);
  const [archived, setArchived] = useState<ArchivedDocument[]>([]);

  const refreshing = useRef(false);

  const refreshLibrary = useCallback(async () => {
    if (refreshing.current) return;
    refreshing.current = true;
    try {
      const [s, d, f, a] = await Promise.all([
        ipc.librarySummary(),
        ipc.listDocuments(),
        ipc.listFolders(),
        ipc.listArchived(),
      ]);
      setSummary(s);
      setDocuments(d);
      setFolders(f);
      setArchived(a);
    } catch (e) {
      setError(formatError(e));
    } finally {
      refreshing.current = false;
    }
  }, [setError]);

  const refreshRecentLibraries = useCallback(async () => {
    try {
      const list = await ipc.listRecentLibraries();
      setRecentLibraries(list);
    } catch {
      // Non-fatal — the switcher just shows whatever it had.
    }
  }, []);

  const clearLibraryUi = useCallback(() => {
    setDocuments(null);
    setFolders([]);
    setArchived([]);
    setSummary(null);
  }, []);

  return {
    libraryOpen,
    libraryPath,
    recentLibraries,
    summary,
    documents,
    folders,
    archived,
    setLibraryOpen,
    setLibraryPath,
    setRecentLibraries,
    setSummary,
    setDocuments,
    setFolders,
    setArchived,
    refreshLibrary,
    refreshRecentLibraries,
    clearLibraryUi,
  };
}

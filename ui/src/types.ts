export interface DocumentSummary {
  document_id: string;
  visible_name: string;
  doc_type: string;
  current_manifest: string;
  current_version_id: number;
}

export interface VersionEntry {
  id: number;
  document_id: string;
  manifest_hash: string;
  parent_version_id: number | null;
  observed_at: string;
  source: "pulled" | "imported" | "restored";
  note: string | null;
}

export interface LibrarySummary {
  path: string;
  document_count: number;
  version_count: number;
  blob_count: number;
  size_bytes: number;
}

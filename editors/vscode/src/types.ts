// Wire types of the annox extension methods (spec §6.6.1).

export interface Position {
  line: number;
  character: number;
}

export interface Range {
  start: Position;
  end: Position;
}

export interface Author {
  id: string;
  name?: string;
}

export interface ConflictEntry {
  event: string;
  author?: Author;
  time?: string;
  value: unknown;
}

export interface Resolution {
  state: string;
  step?: number;
  range?: Range;
}

export interface AnnotationView {
  id: string;
  kind: "comment" | "suggestion";
  author?: Author;
  created?: string;
  body?: string | null;
  label?: string | null;
  status: string;
  edit?: { replacement: string } | null;
  retargetedBy?: { author?: Author } | null;
  deleted?: boolean;
  resolution?: Resolution;
  applicable?: boolean;
  local?: boolean;
  conflicts?: Record<string, ConflictEntry[]>;
  replies?: AnnotationView[];
}

export interface DocumentInfo {
  documents: string[];
  conflicts: { path?: ConflictEntry[] };
  duplicates: boolean;
}

export interface AnnotationsResult {
  annotations: AnnotationView[];
  document: DocumentInfo;
}

export interface DidChangeAnnotations extends AnnotationsResult {
  textDocument: { uri: string };
}

export interface Peer {
  author?: Author;
  textDocument?: { uri: string };
  range?: Range;
}

export interface HistoryEvent {
  type: string;
  time?: string;
  author?: Author;
  body?: string | null;
  status?: string;
  edit?: { replacement: string };
  [field: string]: unknown;
}

export type AcceptResult = { annotation: string; accepted: true } | { annotation: string; error: unknown };

/** The result of `annox/commit`. */
export interface CommitResult {
  /** The new commit, or null if nothing was committed. */
  commit: string | null;
  /** How many files were or would be committed. */
  files: number;
  /** What the events among them do, by document path. */
  documents: Record<string, { comments: number; suggestions: number; replies: number; updates: number }>;
  message: string | null;
}

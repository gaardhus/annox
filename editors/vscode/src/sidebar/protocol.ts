// What crosses between the extension and the sidebar webview. Free of the
// VS Code API, so that the webview bundle and the unit tests can use it.

/** Where a card is listed. */
export type Group = "attention" | "drafts" | "open" | "closed";

/** What a card's buttons do, each the annox command of the same name. */
export type Action =
  | "open"
  | "reply"
  | "resolve"
  | "accept"
  | "reject"
  | "edit"
  | "publish"
  | "reopen"
  | "revert"
  | "conflicts"
  | "history";

/** A stretch of a suggestion's word diff: kept, deleted or inserted. */
export interface Segment {
  op: "=" | "-" | "+";
  text: string;
}

export interface Reply {
  author: string;
  when: string;
  body: string;
}

export interface Card {
  id: string;
  group: Group;
  kind: "comment" | "highlight" | "suggestion";
  status: string;
  draft: boolean;
  author: string;
  when: string;
  /** 1-based, if the annotation has a place in the text. */
  line?: number;
  /** The text a comment is on. */
  quote?: string;
  body?: string;
  /** A suggestion's change, from the text it replaces to its replacement. */
  diff?: Segment[];
  /** Warnings and links to other threads, as plain text. */
  notes: string[];
  replies: Reply[];
  actions: Action[];
}

export interface State {
  /** The active file's name, if there is one. */
  file?: string;
  inWorkspace: boolean;
  cards: Card[];
  /** The cards whose text has the cursor. */
  selected: string[];
  /** How many suggestions Accept All would accept, and drafts Publish All would publish. */
  acceptable: number;
  drafts: number;
}

/** Commands of the whole file, from the sidebar's header. */
export type Command = "init" | "acceptAll" | "publishAll";

/** extension → webview */
export type HostMessage = { type: "state"; state: State } | { type: "select"; ids: string[] };

/** webview → extension */
export type ViewMessage =
  | { type: "ready" }
  | { type: "action"; id: string; action: Action }
  | { type: "reply"; id: string; body: string }
  | { type: "command"; command: Command };

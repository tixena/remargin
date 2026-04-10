export interface CommentOpts {
  replyTo?: string;
  afterLine?: number;
  afterComment?: string;
  to?: string[];
  attachments?: string[];
  autoAck?: boolean;
}

export interface QueryOpts {
  pending?: boolean;
  pendingFor?: string;
  author?: string;
  since?: string;
  expanded?: boolean;
  commentId?: string;
}

export interface GetOpts {
  startLine?: number;
  endLine?: number;
  lineNumbers?: boolean;
}

export interface WriteOpts {
  create?: boolean;
  raw?: boolean;
}

export interface SearchOpts {
  path?: string;
  scope?: "all" | "body" | "comments";
  regex?: boolean;
  ignoreCase?: boolean;
  context?: number;
}

export interface BatchCommentOp {
  content: string;
  replyTo?: string;
  afterLine?: number;
  afterComment?: string;
  to?: string[];
  autoAck?: boolean;
}

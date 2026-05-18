export interface TenantContext {
  slug: string;
  /** True when the user is authenticated for this tenant. */
  authed: boolean;
}

export interface Skill {
  slug: string;
  version: string;
  description: string;
  when_to_use?: string | null;
  tags: string[];
  status: string;
  created_at: string;
  /** Cosine similarity to the semantic query, when one was supplied. */
  similarity?: number | null;
}

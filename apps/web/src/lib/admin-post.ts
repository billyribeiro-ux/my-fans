import { isCreator, isPost, type FeedCreator, type FeedPost } from './contracts.ts';

export interface AdminPostInspection {
  viewer_id: string;
  mode: 'sandbox' | 'production';
  creator: FeedCreator;
  post: FeedPost & {
    state: 'published' | 'draft' | 'scheduled' | 'removed';
    moderation_blocked: boolean;
    media_state: 'processing' | 'quarantine' | 'approved' | 'rejected' | null;
  };
}

export function parseAdminPost(
  value: unknown,
  viewerId: string,
  postId: string,
): AdminPostInspection {
  const invalid = () => {
    throw new Error('Post inspection could not be verified. Refresh to retry.');
  };
  if (!value || typeof value !== 'object' || Array.isArray(value)) return invalid();
  const item = value as Record<string, unknown>;
  if (
    item.viewer_id !== viewerId ||
    !viewerId ||
    !['sandbox', 'production'].includes(String(item.mode)) ||
    !isCreator(item.creator) ||
    !isPost(item.post)
  )
    return invalid();
  const post = item.post as FeedPost & Record<string, unknown>;
  if (
    post.id !== postId ||
    post.creator_id !== item.creator.id ||
    post.locked ||
    !['published', 'draft', 'scheduled', 'removed'].includes(String(post.state)) ||
    typeof post.moderation_blocked !== 'boolean' ||
    ![null, 'processing', 'quarantine', 'approved', 'rejected'].includes(
      post.media_state as string | null,
    )
  )
    return invalid();
  if (
    post.media_state !== null &&
    !['quarantine', 'approved'].includes(String(post.media_state)) &&
    (post.media_url || post.captions_url || post.hls_url || post.preview_url)
  )
    return invalid();
  // Inspection never loads arbitrary third-party media. Seed fixtures use this image host.
  for (const [url, kind] of [
    [post.media_url, 'full'],
    [post.preview_url, 'preview'],
    [post.captions_url, 'captions'],
  ] as const) {
    if (!url) continue;
    if (new RegExp(`^/api/platform/media/[a-f0-9-]{36}/${kind}$`).test(url)) continue;
    try {
      const external = new URL(url);
      if (
        kind !== 'captions' &&
        external.protocol === 'https:' &&
        external.hostname === 'images.unsplash.com' &&
        !external.username &&
        !external.password &&
        !external.port
      )
        continue;
    } catch {
      /* A malformed response must not become a media request. */
    }
    return invalid();
  }
  return item as unknown as AdminPostInspection;
}

import { describe, expect, it } from 'vitest';
import { parseAdminPost } from './admin-post.ts';
const viewerId = 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa';
const postId = 'bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb';
const creatorId = 'cccccccc-cccc-4ccc-8ccc-cccccccccccc';
const mediaId = 'dddddddd-dddd-4ddd-8ddd-dddddddddddd';
const fixture = {
  viewer_id: viewerId,
  mode: 'sandbox',
  creator: {
    id: creatorId,
    user_id: creatorId,
    handle: 'creator',
    display_name: 'Creator',
    category: 'Art',
    bio: '',
    avatar_url: '',
    cover_url: '',
    currency: 'USD',
    subscription_price_minor: 1000,
    subscriber_count: 1,
    subscribed: false,
    verification_status: 'approved',
  },
  post: {
    id: postId,
    creator_id: creatorId,
    caption: 'Reported paid content',
    created_at: '2026-10-08T00:00:00Z',
    media_url: `/api/platform/media/${mediaId}/full`,
    preview_url: `/api/platform/media/${mediaId}/preview`,
    media_type: 'video',
    captions_url: `/api/platform/media/${mediaId}/captions`,
    hls_url: null,
    visibility: 'subscribers',
    price_minor: null,
    liked: false,
    saved: false,
    locked: false,
    like_count: 0,
    comment_count: 0,
    state: 'published',
    moderation_blocked: false,
    media_state: 'approved',
  },
};
describe('private moderator inspection boundary', () => {
  it('accepts authorized paid-media inspection without creating an entitlement', () => {
    const result = parseAdminPost(fixture, viewerId, postId);
    expect(result.post.media_url).toBe(fixture.post.media_url);
    expect(result.creator.subscribed).toBe(false);
  });
  it('rejects stale identity, another post, and a mismatched creator', () => {
    expect(() => parseAdminPost(fixture, creatorId, postId)).toThrow();
    expect(() => parseAdminPost(fixture, viewerId, creatorId)).toThrow();
    expect(() =>
      parseAdminPost(
        { ...fixture, post: { ...fixture.post, creator_id: viewerId } },
        viewerId,
        postId,
      ),
    ).toThrow();
  });
  it('rejects locked, malformed and nonrenderable media responses', () => {
    for (const patch of [
      { locked: true },
      { state: 'unknown' },
      { media_state: 'rejected' },
      { created_at: 'bad' },
      { moderation_blocked: undefined },
    ]) {
      expect(() =>
        parseAdminPost({ ...fixture, post: { ...fixture.post, ...patch } }, viewerId, postId),
      ).toThrow();
    }
    expect(
      parseAdminPost(
        {
          ...fixture,
          post: {
            ...fixture.post,
            media_state: 'rejected',
            media_url: null,
            preview_url: '',
            captions_url: null,
          },
        },
        viewerId,
        postId,
      ).post.media_url,
    ).toBeNull();
  });
  it('does not load arbitrary external media or captions', () => {
    for (const patch of [
      { media_url: 'https://attacker.test/private' },
      { media_url: 'javascript:alert(1)' },
      { captions_url: 'https://images.unsplash.com/captions' },
      { media_url: 'https://user:password@images.unsplash.com/private' },
    ]) {
      expect(() =>
        parseAdminPost({ ...fixture, post: { ...fixture.post, ...patch } }, viewerId, postId),
      ).toThrow();
    }
    expect(
      parseAdminPost(
        {
          ...fixture,
          post: {
            ...fixture.post,
            media_type: 'image',
            media_state: null,
            captions_url: null,
            media_url: 'https://images.unsplash.com/photo-fixture',
            preview_url: 'https://images.unsplash.com/photo-preview',
          },
        },
        viewerId,
        postId,
      ).post.media_url,
    ).toContain('images.unsplash.com');
  });
});

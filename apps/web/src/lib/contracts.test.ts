import { describe, expect, it } from 'vitest';
import { emptyFeed, isFeedResponse } from './contracts';

const post = {
  id: 'post-1',
  creator_id: 'creator-1',
  caption: 'Member content',
  created_at: '2026-10-06T12:00:00Z',
  media_url: null,
  preview_url: 'https://images.example/preview.jpg',
  visibility: 'subscribers',
  media_type: 'image',
  captions_url: null,
  locked: true,
  price_minor: null,
  liked: false,
  saved: false,
  like_count: 1,
  comment_count: 0,
};

describe('feed trust boundary', () => {
  it('accepts empty and locked feeds', () => {
    expect(isFeedResponse(emptyFeed)).toBe(true);
    expect(isFeedResponse({ ...emptyFeed, posts: [post] })).toBe(true);
  });
  it('rejects protected URLs on locked posts', () => {
    expect(isFeedResponse({ ...emptyFeed, posts: [{ ...post, media_url: 'private.jpg' }] })).toBe(
      false,
    );
  });
  it('rejects invalid pagination, viewer roles, and protected captions', () => {
    expect(isFeedResponse({ ...emptyFeed, next_cursor: 'x'.repeat(91) })).toBe(false);
    expect(isFeedResponse({ ...emptyFeed, viewer: { ...emptyFeed.viewer, role: 'owner' } })).toBe(
      false,
    );
    expect(
      isFeedResponse({ ...emptyFeed, posts: [{ ...post, captions_url: 'private.vtt' }] }),
    ).toBe(false);
  });
  it('rejects malformed collections and timestamps', () => {
    expect(isFeedResponse({ ...emptyFeed, posts: null })).toBe(false);
    expect(isFeedResponse({ ...emptyFeed, posts: [{ ...post, created_at: 'invalid' }] })).toBe(
      false,
    );
    expect(isFeedResponse({ ...emptyFeed, posts: [{ ...post, like_count: -1 }] })).toBe(false);
  });
  it('rejects malformed creator prices and currencies', () => {
    const creator = {
      id: 'creator',
      user_id: 'user',
      verification_status: 'approved',
      handle: 'creator',
      display_name: 'Creator',
      category: 'Art',
      bio: '',
      avatar_url: '',
      cover_url: '',
      currency: 'USD',
      subscription_price_minor: 1200,
      subscriber_count: 12,
      subscribed: false,
    };
    expect(isFeedResponse({ ...emptyFeed, creators: [creator] })).toBe(true);
    expect(
      isFeedResponse({ ...emptyFeed, creators: [{ ...creator, currency: 'not a currency' }] }),
    ).toBe(false);
    expect(
      isFeedResponse({ ...emptyFeed, creators: [{ ...creator, subscription_price_minor: 12.5 }] }),
    ).toBe(false);
  });
});

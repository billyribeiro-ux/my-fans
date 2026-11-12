export interface FeedCreator {
  id: string;
  user_id: string;
  verification_status: 'pending' | 'approved' | 'rejected';
  handle: string;
  display_name: string;
  category: string;
  bio: string;
  avatar_url: string;
  cover_url: string;
  subscription_price_minor: number;
  currency: string;
  subscriber_count: number;
  subscribed: boolean;
}

export interface FeedPost {
  id: string;
  creator_id: string;
  caption: string;
  created_at: string;
  media_url: string | null;
  preview_url: string;
  media_type: 'image' | 'video';
  hls_url?: string | null;
  captions_url: string | null;
  visibility: 'public' | 'subscribers' | 'purchase';
  price_minor: number | null;
  liked: boolean;
  saved: boolean;
  locked: boolean;
  like_count: number;
  comment_count: number;
}

export interface User {
  id: string;
  email: string;
  display_name: string;
  avatar_url: string;
  role: 'fan' | 'creator' | 'admin' | 'guest';
  email_verified: boolean;
  mfa_enabled: boolean;
}

export interface FeedResponse {
  viewer: User;
  creators: FeedCreator[];
  posts: FeedPost[];
  next_cursor: string | null;
  mode: 'sandbox' | 'production';
}

export const emptyFeed: FeedResponse = {
  viewer: {
    id: '',
    email: '',
    display_name: 'Guest',
    avatar_url: '',
    role: 'guest',
    email_verified: false,
    mfa_enabled: false,
  },
  creators: [],
  posts: [],
  next_cursor: null,
  mode: 'sandbox',
};

// Validate the external boundary rather than trusting a TypeScript assertion.
export function isFeedResponse(value: unknown): value is FeedResponse {
  if (
    !isRecord(value) ||
    !['sandbox', 'production'].includes(String(value.mode)) ||
    !isRecord(value.viewer)
  )
    return false;
  if (!(
    value.next_cursor === null ||
    (typeof value.next_cursor === 'string' && value.next_cursor.length <= 90)
  ))
    return false;
  const viewer = value.viewer;
  if (
    !['id', 'email', 'display_name', 'avatar_url'].every((key) => typeof viewer[key] === 'string')
  )
    return false;
  if (
    !['fan', 'creator', 'admin', 'guest'].includes(String(viewer.role)) ||
    typeof viewer.email_verified !== 'boolean' ||
    typeof viewer.mfa_enabled !== 'boolean'
  )
    return false;
  if (!Array.isArray(value.creators) || !Array.isArray(value.posts)) return false;
  return value.creators.every(isCreator) && value.posts.every(isPost);
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

export function isCreator(value: unknown): value is FeedCreator {
  return (
    isRecord(value) &&
    [
      'id',
      'user_id',
      'handle',
      'display_name',
      'category',
      'bio',
      'avatar_url',
      'cover_url',
      'currency',
    ].every((key) => typeof value[key] === 'string') &&
    ['pending', 'approved', 'rejected'].includes(String(value.verification_status)) &&
    typeof value.subscribed === 'boolean' &&
    typeof value.currency === 'string' &&
    /^[A-Z]{3}$/.test(value.currency) &&
    Number.isSafeInteger(value.subscription_price_minor) &&
    Number(value.subscription_price_minor) >= 0 &&
    Number.isSafeInteger(value.subscriber_count) &&
    Number(value.subscriber_count) >= 0
  );
}

export function isPost(value: unknown): value is FeedPost {
  return (
    isRecord(value) &&
    ['id', 'creator_id', 'caption', 'created_at', 'preview_url'].every(
      (key) => typeof value[key] === 'string',
    ) &&
    typeof value.created_at === 'string' &&
    Number.isFinite(Date.parse(value.created_at)) &&
    ['public', 'subscribers', 'purchase'].includes(String(value.visibility)) &&
    ['image', 'video'].includes(String(value.media_type)) &&
    typeof value.locked === 'boolean' &&
    typeof value.liked === 'boolean' &&
    typeof value.saved === 'boolean' &&
    (value.price_minor === null ||
      (Number.isSafeInteger(value.price_minor) && Number(value.price_minor) > 0)) &&
    (value.media_url === null || typeof value.media_url === 'string') &&
    (value.captions_url === null || typeof value.captions_url === 'string') &&
    (value.hls_url === undefined || value.hls_url === null || typeof value.hls_url === 'string') &&
    (!value.locked || value.hls_url === undefined || value.hls_url === null) &&
    (!value.locked || value.captions_url === null) &&
    (!value.locked || value.media_url === null) &&
    Number.isSafeInteger(value.like_count) &&
    Number(value.like_count) >= 0 &&
    Number.isSafeInteger(value.comment_count) &&
    Number(value.comment_count) >= 0
  );
}

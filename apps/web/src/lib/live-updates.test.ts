import { describe, expect, it } from 'vitest';
import { parseThread, parseNotifications } from './live-updates.ts';
const conversation = {
  id: 'thread',
  recipient_id: 'recipient',
  display_name: 'Recipient',
  last_message: null,
  updated_at: '2026-10-07T00:00:00Z',
  blocked: false,
  own_blocked: false,
};
const envelope = { viewer_id: 'viewer', conversation, messages: [], next_cursor: null };
describe('live update identity and collection boundaries', () => {
  it('requires authoritative restriction flags on the exact selected thread', () => {
    expect(
      parseThread(
        { ...envelope, conversation: { ...conversation, blocked: true } },
        'thread',
        'viewer',
      ).conversation.blocked,
    ).toBe(true);
    expect(() => parseThread(envelope, 'another-thread', 'viewer')).toThrow();
    expect(() =>
      parseThread(
        { ...envelope, conversation: { ...conversation, own_blocked: undefined } },
        'thread',
        'viewer',
      ),
    ).toThrow();
  });
  it('rejects updates belonging to another authenticated account', () => {
    expect(() => parseThread(envelope, 'thread', 'another-viewer')).toThrow(
      'Account identity changed',
    );
    expect(() =>
      parseNotifications(
        { viewer_id: 'viewer', notifications: [], next_cursor: null },
        'another-viewer',
      ),
    ).toThrow('Account identity changed');
  });
  it('bounds incoming pages and rejects duplicate message identities', () => {
    const message = {
      id: 'message',
      sender_id: 'recipient',
      body: 'Message',
      created_at: '2026-10-07T00:00:00Z',
    };
    expect(() =>
      parseThread({ ...envelope, messages: [message, message] }, 'thread', 'viewer'),
    ).toThrow();
    expect(() =>
      parseThread({ ...envelope, messages: Array(51).fill(message) }, 'thread', 'viewer'),
    ).toThrow();
  });
  it('preserves unread state and authoritative end-of-page null cursors', () => {
    const item = {
      id: 'notification',
      title: 'New message',
      body: 'A new message arrived.',
      read_at: null,
      created_at: '2026-10-07T00:00:00Z',
    };
    expect(
      parseNotifications(
        { viewer_id: 'viewer', notifications: [item], next_cursor: null },
        'viewer',
      ),
    ).toEqual({ notifications: [item], next_cursor: null });
    expect(() =>
      parseNotifications(
        {
          viewer_id: 'viewer',
          notifications: [{ ...item, read_at: 'invalid-date' }],
          next_cursor: null,
        },
        'viewer',
      ),
    ).toThrow();
    expect(() =>
      parseNotifications(
        { viewer_id: 'viewer', notifications: [item, item], next_cursor: null },
        'viewer',
      ),
    ).toThrow();
  });
});

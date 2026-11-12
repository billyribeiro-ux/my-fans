export interface Conversation {
  id: string;
  recipient_id: string;
  display_name: string;
  last_message: string | null;
  updated_at: string;
  blocked: boolean;
  own_blocked: boolean;
}
export interface ThreadMessage {
  id: string;
  sender_id: string;
  body: string;
  created_at: string;
}
export interface Notification {
  id: string;
  title: string;
  body: string;
  read_at: string | null;
  created_at: string;
}
const record = (v: unknown): v is Record<string, unknown> =>
  !!v && typeof v === 'object' && !Array.isArray(v);
const text = (v: unknown): v is string => typeof v === 'string';
const date = (v: unknown) => text(v) && Number.isFinite(Date.parse(v));
const cursor = (v: unknown) => v === null || (text(v) && v.length <= 90);
export function parseThread(value: unknown, expectedId: string, expectedViewer: string) {
  if (!record(value) || value.viewer_id !== expectedViewer)
    throw new Error('Account identity changed. Refresh your account before continuing.');
  if (
    !record(value) ||
    !Array.isArray(value.messages) ||
    value.messages.length > 50 ||
    !cursor(value.next_cursor) ||
    !record(value.conversation)
  )
    throw new Error('Conversation updates could not be verified.');
  const conversation = value.conversation;
  if (
    conversation.id !== expectedId ||
    !text(conversation.recipient_id) ||
    !text(conversation.display_name) ||
    !(conversation.last_message === null || text(conversation.last_message)) ||
    !date(conversation.updated_at) ||
    typeof conversation.blocked !== 'boolean' ||
    typeof conversation.own_blocked !== 'boolean' ||
    !value.messages.every(
      (m) => record(m) && text(m.id) && text(m.sender_id) && text(m.body) && date(m.created_at),
    ) ||
    new Set(value.messages.map((m) => m.id)).size !== value.messages.length
  )
    throw new Error('Conversation updates could not be verified.');
  return {
    messages: value.messages as ThreadMessage[],
    next_cursor: value.next_cursor as string | null,
    conversation: conversation as unknown as Conversation,
  };
}
export function parseNotifications(value: unknown, expectedViewer: string) {
  if (!record(value) || value.viewer_id !== expectedViewer)
    throw new Error('Account identity changed. Refresh your account before continuing.');
  if (
    !record(value) ||
    !Array.isArray(value.notifications) ||
    value.notifications.length > 50 ||
    !cursor(value.next_cursor) ||
    !value.notifications.every(
      (n) =>
        record(n) &&
        text(n.id) &&
        text(n.title) &&
        text(n.body) &&
        date(n.created_at) &&
        (n.read_at === null || date(n.read_at)),
    ) ||
    new Set(value.notifications.map((n) => n.id)).size !== value.notifications.length
  )
    throw new Error('Notification updates could not be verified.');
  return {
    notifications: value.notifications as Notification[],
    next_cursor: value.next_cursor as string | null,
  };
}

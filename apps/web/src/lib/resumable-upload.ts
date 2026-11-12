export const uploadChunkSize = 4 * 1024 * 1024;
export const imageLimit = 10 * 1024 * 1024;
export const videoLimit = 256 * 1024 * 1024;
const uuid = /^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$/;
export const uploadTypes = ['image/jpeg', 'image/png', 'image/webp', 'video/mp4', 'video/webm'];
export interface UploadSession {
  id: string;
  offset: number;
  size_bytes: number;
  content_type: string;
  chunk_size: number;
  state: 'uploading' | 'assembling' | 'complete' | 'cancelled';
  expires_at: string;
}
export interface SavedUpload {
  id: string;
  size: number;
  type: string;
  lastModified: number;
  fingerprint: string;
}
export function validateUploadFile(file: Pick<File, 'size' | 'type'>): void {
  if (!uploadTypes.includes(file.type))
    throw new Error('Choose a JPEG, PNG, WebP, MP4, or WebM file.');
  if (
    !Number.isSafeInteger(file.size) ||
    file.size <= 0 ||
    file.size > (file.type.startsWith('video/') ? videoLimit : imageLimit)
  )
    throw new Error(
      file.type.startsWith('video/')
        ? 'Videos must be no larger than 256 MiB.'
        : 'Images must be no larger than 10 MiB.',
    );
}
export function parseUpload(
  value: unknown,
  expected?: Pick<SavedUpload, 'id' | 'size' | 'type'>,
): UploadSession {
  const session = (value as { upload?: UploadSession } | null)?.upload;
  if (
    !session ||
    !uuid.test(session.id) ||
    !uploadTypes.includes(session.content_type) ||
    !Number.isSafeInteger(session.size_bytes) ||
    session.size_bytes <= 0 ||
    session.size_bytes > (session.content_type.startsWith('video/') ? videoLimit : imageLimit) ||
    !Number.isSafeInteger(session.offset) ||
    session.offset < 0 ||
    session.offset > session.size_bytes ||
    session.chunk_size !== uploadChunkSize ||
    !['uploading', 'assembling', 'complete', 'cancelled'].includes(session.state) ||
    typeof session.expires_at !== 'string' ||
    !Number.isFinite(Date.parse(session.expires_at)) ||
    (expected &&
      (session.id !== expected.id ||
        session.size_bytes !== expected.size ||
        session.content_type !== expected.type))
  )
    throw new Error('The saved upload could not be verified. Cancel it and start again.');
  return session;
}
export function parseSavedUpload(value: unknown): SavedUpload {
  const saved = value as SavedUpload | null;
  if (
    !saved ||
    !uuid.test(saved.id) ||
    !uploadTypes.includes(saved.type) ||
    !Number.isSafeInteger(saved.lastModified) ||
    saved.lastModified < 0 ||
    !/^[a-f0-9]{64}$/.test(saved.fingerprint)
  )
    throw new Error('Invalid saved upload.');
  validateUploadFile({ size: saved.size, type: saved.type });
  return {
    id: saved.id,
    size: saved.size,
    type: saved.type,
    lastModified: saved.lastModified,
    fingerprint: saved.fingerprint,
  };
}
/** Hash metadata only; no file bytes or filename are persisted or sent to the API. */
export async function fileFingerprint(
  file: Pick<File, 'name' | 'size' | 'type' | 'lastModified'>,
): Promise<string> {
  const bytes = new TextEncoder().encode(
    JSON.stringify([file.name, file.size, file.type, file.lastModified]),
  );
  const digest = await crypto.subtle.digest('SHA-256', bytes);
  return [...new Uint8Array(digest)].map((byte) => byte.toString(16).padStart(2, '0')).join('');
}

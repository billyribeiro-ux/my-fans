import { describe, expect, it } from 'vitest';
import {
  imageLimit,
  parseSavedUpload,
  parseUpload,
  uploadChunkSize,
  validateUploadFile,
  videoLimit,
} from './resumable-upload.ts';
const id = 'aabbccdd-1234-5678-9000-aabbccddeeff';
const session = {
  id,
  offset: 4194304,
  size_bytes: 6000000,
  chunk_size: uploadChunkSize,
  content_type: 'video/mp4',
  state: 'uploading',
  expires_at: '2026-10-08T00:00:00Z',
};
describe('resumable upload boundary', () => {
  it('accepts bounded image/video metadata and valid server offsets', () => {
    validateUploadFile({ size: imageLimit, type: 'image/jpeg' });
    validateUploadFile({ size: videoLimit, type: 'video/mp4' });
    expect(parseUpload({ upload: session }, { id, size: 6000000, type: 'video/mp4' }).offset).toBe(
      uploadChunkSize,
    );
  });
  it('rejects oversized files and unsafe or mismatched session counters', () => {
    expect(() => validateUploadFile({ size: imageLimit + 1, type: 'image/png' })).toThrow();
    expect(() => validateUploadFile({ size: videoLimit + 1, type: 'video/mp4' })).toThrow();
    expect(() => validateUploadFile({ size: 100, type: 'image/svg+xml' })).toThrow();
    for (const patch of [
      { offset: -1 },
      { offset: 6000001 },
      { chunk_size: 8000000 },
      { content_type: 'text/html' },
      { state: 'approved' },
    ])
      expect(() => parseUpload({ upload: { ...session, ...patch } })).toThrow();
    expect(() =>
      parseUpload({ upload: session }, { id, size: 5000000, type: 'video/mp4' }),
    ).toThrow();
  });
  it('retains only metadata and a filename fingerprint in saved progress', () => {
    const saved = {
      id,
      size: 6000000,
      type: 'video/mp4',
      lastModified: 1000,
      fingerprint: 'a'.repeat(64),
      filename: 'private.mp4',
      content: 'secret',
    };
    expect(parseSavedUpload(saved)).not.toHaveProperty('filename');
    expect(parseSavedUpload(saved)).not.toHaveProperty('content');
    expect(() => parseSavedUpload({ ...saved, fingerprint: 'private.mp4' })).toThrow();
  });
});

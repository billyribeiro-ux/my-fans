/** SvelteKit development access logs include query strings unless the Vite logger redacts them. */
export function redactRequestLog(message: string): string {
  const plain = message.replace(/\x1b\[[0-9;]*m/g, '');
  const request = plain.match(/^(\d{3} [A-Z]+ )(\S+)(?:.*)$/s);
  if (!request) return message;
  return request[1] + request[2].split(/[?#]/, 1)[0];
}

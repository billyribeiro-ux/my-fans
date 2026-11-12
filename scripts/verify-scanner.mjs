import { connect } from 'node:net';
function command(bytes) {
  return new Promise((resolve, reject) => {
    const socket = connect({ host: '127.0.0.1', port: 3310 });
    let result = Buffer.alloc(0);
    const deadline = setTimeout(
      () => socket.destroy(new Error('Scanner exceeded its 30-second application deadline')),
      30000,
    );
    socket.once('close', () => clearTimeout(deadline));
    socket.on('connect', async () => {
      try {
        if (typeof bytes === 'function') await bytes(socket);
        else socket.write(bytes);
      } catch (error) {
        socket.destroy(error);
      }
    });
    socket.on('error', reject);
    socket.on('data', (chunk) => {
      result = Buffer.concat([result, chunk]);
      if (result.length > 4096)
        return socket.destroy(new Error('Scanner response exceeds its bound'));
      const end = result.indexOf(0);
      if (end >= 0) {
        socket.destroy();
        resolve(result.subarray(0, end).toString());
      }
    });
    socket.on('end', () => reject(new Error('Scanner closed without a complete response')));
  });
}
function write(socket, bytes) {
  // Wait for each write callback before producing the next chunk. Memory stays
  // bounded even when the scanner applies backpressure during a large stream.
  return new Promise((resolve, reject) =>
    socket.write(bytes, (error) => (error ? reject(error) : resolve())),
  );
}
async function scanMaximumVideoBytes() {
  const frame = Buffer.alloc(4 + 64 * 1024);
  frame.writeUInt32BE(64 * 1024);
  const started = performance.now();
  const result = await command(async (socket) => {
    await write(socket, Buffer.from('zINSTREAM\0'));
    for (let offset = 0; offset < 256 * 1024 * 1024; offset += 64 * 1024)
      await write(socket, frame);
    await write(socket, Buffer.alloc(4));
  });
  if (result !== 'stream: OK') throw new Error('The bounded 256 MiB clean stream was not accepted');
  return Math.round(performance.now() - started);
}
function scan(text) {
  const input = Buffer.from(text),
    length = Buffer.alloc(4);
  length.writeUInt32BE(input.length);
  return command(Buffer.concat([Buffer.from('zINSTREAM\0'), length, input, Buffer.alloc(4)]));
}
const version = await command(Buffer.from('zVERSION\0'));
const date = Date.parse(version.split('/').slice(2).join('/') + ' UTC');
if (!Number.isFinite(date) || Date.now() - date > 48 * 3600000 || date - Date.now() > 300000)
  throw new Error('Scanner signatures are stale, unknown, or in the future');
if ((await scan('MyFans clean acceptance fixture')) !== 'stream: OK')
  throw new Error('Clean input was not accepted');
const harmless = ['X5O!P%@AP[4\\PZX54(P^)7CC)7}$', 'EICAR-STANDARD-ANTIVIRUS-TEST-FILE!$H+H*'].join(
  '',
);
if (!(await scan(harmless)).endsWith(' FOUND'))
  throw new Error('The harmless antivirus test fixture was not rejected');
const maximumStreamMs = await scanMaximumVideoBytes();
console.log(
  `Fresh scanner signatures; clean fixture accepted; harmless antivirus fixture rejected; 256 MiB benign stream accepted in ${maximumStreamMs} ms.`,
);

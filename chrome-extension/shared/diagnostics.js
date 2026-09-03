const MAX_CLOSE_REASON_BYTES = 123;

export function webSocketCloseReason(reason) {
  const cleaned = String(reason || 'ended').replace(/[\u0000-\u001f\u007f]/g, ' ');
  const codePoints = [...`session: ${cleaned}`];
  const encoder = new TextEncoder();
  while (encoder.encode(codePoints.join('')).byteLength > MAX_CLOSE_REASON_BYTES) {
    codePoints.pop();
  }
  return codePoints.join('');
}

export const MAX_AUDIO_BACKLOG_BYTES = 256 * 1024;

export function floatToS16(value) {
  const finite = Number.isFinite(value) ? value : 0;
  const clamped = Math.max(-1, Math.min(1, finite));
  return clamped < 0 ? Math.round(clamped * 32768) : Math.round(clamped * 32767);
}

export function shouldDropAudio(bufferedAmount, packetBytes) {
  if (!Number.isFinite(bufferedAmount) || bufferedAmount < 0) return true;
  if (!Number.isInteger(packetBytes) || packetBytes <= 0) return true;
  return bufferedAmount + packetBytes > MAX_AUDIO_BACKLOG_BYTES;
}

export class PcmPacketizer {
  #framesPerPacket;
  #left;
  #right;
  #length = 0;

  constructor(framesPerPacket = 960) {
    if (!Number.isInteger(framesPerPacket) || framesPerPacket <= 0) {
      throw new RangeError('frames per packet must be a positive integer');
    }
    this.#framesPerPacket = framesPerPacket;
    this.#left = new Float32Array(framesPerPacket);
    this.#right = new Float32Array(framesPerPacket);
  }

  push(left, right) {
    if (!(left instanceof Float32Array) || !(right instanceof Float32Array)) {
      throw new TypeError('channels must be Float32Array values');
    }
    if (left.length !== right.length) {
      throw new RangeError('channel lengths must match');
    }

    const packets = [];
    let sourceOffset = 0;
    while (sourceOffset < left.length) {
      const count = Math.min(this.#framesPerPacket - this.#length, left.length - sourceOffset);
      this.#left.set(left.subarray(sourceOffset, sourceOffset + count), this.#length);
      this.#right.set(right.subarray(sourceOffset, sourceOffset + count), this.#length);
      this.#length += count;
      sourceOffset += count;

      if (this.#length === this.#framesPerPacket) {
        packets.push(this.#encodePacket());
        this.#length = 0;
      }
    }
    return packets;
  }

  reset() {
    this.#length = 0;
  }

  #encodePacket() {
    const packet = new ArrayBuffer(this.#framesPerPacket * 2 * Int16Array.BYTES_PER_ELEMENT);
    const view = new DataView(packet);
    for (let frame = 0; frame < this.#framesPerPacket; frame += 1) {
      const byteOffset = frame * 4;
      view.setInt16(byteOffset, floatToS16(this.#left[frame]), true);
      view.setInt16(byteOffset + 2, floatToS16(this.#right[frame]), true);
    }
    return packet;
  }
}

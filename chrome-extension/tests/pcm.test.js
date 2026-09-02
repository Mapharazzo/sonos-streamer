import test from 'node:test';
import assert from 'node:assert/strict';

import { PcmPacketizer, floatToS16 } from '../shared/pcm.js';

test('packetizer emits exactly 960 interleaved stereo frames', () => {
  const packetizer = new PcmPacketizer(960);
  const left = new Float32Array(960).fill(-1);
  const right = new Float32Array(960).fill(1);

  const [packet] = packetizer.push(left, right);
  const view = new DataView(packet);

  assert.equal(packet.byteLength, 3840);
  assert.equal(view.getInt16(0, true), -32768);
  assert.equal(view.getInt16(2, true), 32767);
});

test('packetizer carries partial render quanta across pushes', () => {
  const packetizer = new PcmPacketizer(4);

  assert.deepEqual(
    packetizer.push(new Float32Array([0, 0.25]), new Float32Array([0.5, 0.75])),
    [],
  );
  const [packet] = packetizer.push(
    new Float32Array([-0.25, -1]),
    new Float32Array([-0.5, 1]),
  );
  const samples = new Int16Array(packet);

  assert.deepEqual([...samples], [0, 16384, 8192, 24575, -8192, -16384, -32768, 32767]);
});

test('reset discards a partial packet and non-finite samples become silence', () => {
  const packetizer = new PcmPacketizer(2);
  packetizer.push(new Float32Array([1]), new Float32Array([1]));
  packetizer.reset();

  assert.equal(floatToS16(Number.NaN), 0);
  assert.equal(packetizer.push(new Float32Array([0]), new Float32Array([0])).length, 0);
});

test('packetizer rejects mismatched channel lengths', () => {
  const packetizer = new PcmPacketizer(960);
  assert.throws(
    () => packetizer.push(new Float32Array(2), new Float32Array(1)),
    /channel lengths/i,
  );
});

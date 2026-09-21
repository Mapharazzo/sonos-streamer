import { BRIDGE_URL, createCommand, createHello, parseServerEvent } from './shared/protocol.js';
import { PcmPacketizer, audioBacklogAction } from './shared/pcm.js';
import { GenerationGate } from './shared/generation-gate.js';
import { webSocketCloseReason } from './shared/diagnostics.js';
import { ManagedClient } from './shared/managed-client.js';
import { AUDIO_URL, AUDIO_FORMAT, createManagedCommand } from './shared/managed-protocol.js';
import { reduceManagedState } from './shared/managed-state.js';

const packetizer = new PcmPacketizer(960);
const captureGate = new GenerationGate();

let socket = null;
let socketGeneration = 0;
let authenticated = false;
let mediaStream = null;
let audioContext = null;
let mediaSource = null;
let workletNode = null;
let backpressureWarningSent = false;
let endingTrack = null;
let capturedTabId = null;
let sentPackets = 0;
let droppedPackets = 0;
let managedMode = false;
let managedToken = '';
let managedState = null;
let commandQueue = Promise.resolve();
let pendingCapture = null;
const managedClient = new ManagedClient({ onEvent(event) {
  managedState = reduceManagedState(managedState, event);
  if (event.type === 'disconnected' || event.type === 'error') void stopManagedCapture().catch(() => {});
  broadcastManaged(event);
} });

chrome.runtime.onMessage.addListener((message, _sender, sendResponse) => {
  if (!message || typeof message !== 'object') return undefined;

  let operation;
  switch (message.type) {
    case 'managed-connect':
      operation = connectManaged(message.token);
      break;
    case 'managed-state':
      operation = Promise.resolve(managedSnapshot());
      break;
    case 'managed-command':
      operation = commandQueue.then(() => managedCommand(message.action));
      commandQueue = operation.catch(() => {});
      break;
    case 'managed-start':
      if (pendingCapture) operation = Promise.reject(new Error('capture-busy'));
      else {
        operation = startManagedCapture(message); pendingCapture = operation;
        void operation.finally(() => { if (pendingCapture === operation) pendingCapture = null; }).catch(() => {});
      }
      break;
    case 'managed-stop':
      operation = stopManagedCapture();
      break;
    case 'managed-close':
      managedClient.close();
      operation = stopManagedCapture();
      break;
    case 'bridge-connect':
      operation = connectBridge(message.token, message.url);
      break;
    case 'start-capture':
      operation = startCapture(message.streamId, message.tabId);
      break;
    case 'offscreen-state':
      operation = Promise.resolve(offscreenState());
      break;
    case 'stop-session':
      operation = stopSession(message.reason);
      break;
    case 'stop-capture':
      operation = stopCapture();
      break;
    case 'browser-command':
      operation = sendBrowserCommand(message.commandType, message.requestId, message.fields);
      break;
    default:
      return undefined;
  }

  operation.then(
    (value) => sendResponse({ ok: true, value }),
    (error) => sendResponse({ ok: false, error: publicError(error) }),
  );
  return true;
});

async function connectBridge(token, requestedUrl = BRIDGE_URL) {
  managedClient.close();
  await stopManagedCapture();
  managedMode = false;
  if (typeof token !== 'string' || token.length === 0) throw new Error('A pairing token is required');
  const url = validateBridgeUrl(requestedUrl);
  await disconnectSocket('bridge reconnect');
  broadcast({ type: 'bridge-connecting' });

  const generation = ++socketGeneration;
  const nextSocket = new WebSocket(url);
  nextSocket.binaryType = 'arraybuffer';
  socket = nextSocket;

  try {
    await new Promise((resolve, reject) => {
      nextSocket.addEventListener('open', resolve, { once: true });
      nextSocket.addEventListener(
        'error',
        () => reject(new Error('Could not connect to the local Sonos app')),
        { once: true },
      );
      nextSocket.addEventListener(
        'close',
        () => reject(new Error('The local Sonos app closed the connection')),
        { once: true },
      );
    });
  } catch (error) {
    if (socket === nextSocket) socket = null;
    authenticated = false;
    nextSocket.close();
    broadcast({ type: 'bridge-disconnected', message: publicError(error) });
    throw error;
  }
  if (socket !== nextSocket || generation !== socketGeneration) {
    nextSocket.close();
    throw new Error('Bridge connection was superseded');
  }

  installSocketHandlers(nextSocket, generation);
  broadcast({ type: 'bridge-connected' });
  nextSocket.send(JSON.stringify(createHello(token)));
}

function installSocketHandlers(activeSocket, generation) {
  activeSocket.addEventListener('message', (message) => {
    if (activeSocket !== socket || generation !== socketGeneration) return;
    if (typeof message.data !== 'string') {
      broadcast({ type: 'warning', code: 'unexpected-binary', message: 'Ignored binary bridge event' });
      return;
    }
    try {
      const event = parseServerEvent(JSON.parse(message.data));
      if (event.type === 'hello-ok') authenticated = true;
      broadcast(event);
    } catch {
      broadcast({ type: 'warning', code: 'invalid-bridge-event', message: 'Ignored an invalid bridge event' });
    }
  });

  activeSocket.addEventListener('close', () => {
    if (activeSocket !== socket || generation !== socketGeneration) return;
    socket = null;
    authenticated = false;
    void stopCapture();
    broadcast({ type: 'bridge-disconnected', message: 'The local Sonos app disconnected' });
  });

  activeSocket.addEventListener('error', () => {
    if (activeSocket === socket && generation === socketGeneration) {
      broadcast({ type: 'warning', code: 'bridge-socket-error', message: 'Local bridge connection error' });
    }
  });
}

async function startCapture(streamId, tabId) {
  if (!authenticated || socket?.readyState !== WebSocket.OPEN) {
    throw new Error('Pair with the local Sonos app before capturing audio');
  }
  if (typeof streamId !== 'string' || streamId.length === 0) {
    throw new Error('A tab capture stream ID is required');
  }
  if (!Number.isInteger(tabId)) throw new Error('A captured tab ID is required');

  await stopCapture();
  const generation = captureGate.begin();
  let stream = null;
  let context = null;
  let source = null;
  let node = null;
  try {
    stream = await navigator.mediaDevices.getUserMedia({
      audio: {
        mandatory: {
          chromeMediaSource: 'tab',
          chromeMediaSourceId: streamId,
        },
      },
      video: false,
    });
    requireCurrentCapture(generation);

    context = new AudioContext({ sampleRate: 48000 });
    await context.audioWorklet.addModule('audio-worklet.js');
    requireCurrentCapture(generation);
    source = context.createMediaStreamSource(stream);
    node = new AudioWorkletNode(context, 'tab-audio-processor', {
      numberOfInputs: 1,
      numberOfOutputs: 0,
      channelCount: 2,
      channelCountMode: 'explicit',
      channelInterpretation: 'speakers',
    });
    node.port.onmessage = handleAudioQuantum;
    source.connect(node);
    await context.resume();
    requireCurrentCapture(generation);

    mediaStream = stream;
    audioContext = context;
    mediaSource = source;
    workletNode = node;
    endingTrack = stream.getAudioTracks()[0] ?? null;
    if (endingTrack) endingTrack.onended = () => broadcast({ type: 'capture-ended' });
    capturedTabId = tabId;
    packetizer.reset();
    backpressureWarningSent = false;
    sentPackets = 0;
    droppedPackets = 0;
    broadcast({ type: 'capture-started', tabId });
  } catch (error) {
    if (node) {
      node.port.onmessage = null;
      node.disconnect();
    }
    source?.disconnect();
    stream?.getTracks().forEach((track) => track.stop());
    if (context && context.state !== 'closed') await context.close();
    throw error;
  }
}

function handleAudioQuantum(message) {
  if (message.data?.type !== 'audio-quantum') return;
  let packets;
  try {
    packets = packetizer.push(message.data.left, message.data.right);
  } catch {
    broadcast({ type: 'warning', code: 'invalid-audio-quantum', message: 'Ignored malformed captured audio' });
    return;
  }

  for (const packet of packets) {
    const activeSocket = socket;
    if (!authenticated || activeSocket?.readyState !== WebSocket.OPEN) return;
    const backlogAction = audioBacklogAction(activeSocket.bufferedAmount, packet.byteLength);
    if (backlogAction === 'reset') {
      activeSocket.close(1011, 'audio backlog exceeded');
      return;
    }
    if (backlogAction === 'drop') {
      droppedPackets += 1;
      if (!backpressureWarningSent) {
        backpressureWarningSent = true;
        broadcast({
          type: 'warning',
          code: 'audio-backpressure',
          message: 'Network backlog is high; live audio packets are being dropped',
        });
        broadcastAudioStats();
      }
      continue;
    }
    backpressureWarningSent = false;
    activeSocket.send(packet);
    sentPackets += 1;
    if ((sentPackets + droppedPackets) % 50 === 0) broadcastAudioStats();
  }
}

async function sendBrowserCommand(commandType, requestId, fields = {}) {
  if (!authenticated || socket?.readyState !== WebSocket.OPEN) {
    throw new Error('The local Sonos app is not paired');
  }
  socket.send(JSON.stringify(createCommand(commandType, requestId, fields)));
}

async function stopSession(reason = 'ended') {
  await stopCapture();
  await disconnectSocket(reason);
  broadcast({ type: 'bridge-disconnected' });
}

async function stopCapture() {
  captureGate.cancel();
  packetizer.reset();
  if (workletNode) {
    workletNode.port.onmessage = null;
    workletNode.disconnect();
  }
  if (mediaSource) mediaSource.disconnect();
  if (endingTrack) endingTrack.onended = null;
  if (mediaStream) {
    const tracks = mediaStream.getTracks();
    tracks.forEach((track) => track.stop());
    if (managedMode && tracks.some((track) => track.readyState !== 'ended')) throw new Error('capture-cleanup-failed');
  }
  if (audioContext && audioContext.state !== 'closed') await audioContext.close();
  workletNode = null;
  mediaSource = null;
  mediaStream = null;
  audioContext = null;
  endingTrack = null;
  capturedTabId = null;
}

function requireCurrentCapture(generation) {
  if (
    !captureGate.isCurrent(generation) ||
    !authenticated ||
    socket?.readyState !== WebSocket.OPEN
  ) {
    throw new Error('Capture setup was cancelled');
  }
}

function offscreenState() {
  return {
    connected: socket?.readyState === WebSocket.OPEN,
    authenticated,
    capturing: mediaStream !== null,
    tabId: capturedTabId,
  };
}

function broadcastAudioStats() {
  broadcast({ type: 'audio-stats', receivedPackets: sentPackets, droppedPackets });
}

async function disconnectSocket(reason = 'ended') {
  authenticated = false;
  socketGeneration += 1;
  const activeSocket = socket;
  socket = null;
  if (activeSocket && activeSocket.readyState < WebSocket.CLOSING) {
    activeSocket.close(1000, webSocketCloseReason(reason));
  }
}

function validateBridgeUrl(value) {
  const url = new URL(value ?? BRIDGE_URL);
  if (url.protocol !== 'ws:' || url.hostname !== '127.0.0.1' || url.pathname !== '/browser/v1') {
    throw new Error('Bridge URL must use ws://127.0.0.1:<port>/browser/v1');
  }
  return url.href;
}

function broadcast(event) {
  if (managedMode) { broadcastManaged(event); return; }
  void chrome.runtime.sendMessage({ type: 'offscreen-event', event }).catch(() => {});
}

function broadcastManaged(event) {
  void chrome.runtime.sendMessage({ type: 'managed-offscreen-event', event }).catch(() => {});
}
function managedSnapshot() { return { ...managedState, capturing: mediaStream !== null, tabId: capturedTabId, counters: { receivedPackets: sentPackets, droppedPackets } }; }
async function connectManaged(token) {
  if (!/^[a-fA-F0-9]{64}$/.test(token)) throw new Error('invalid-pairing-token');
  if (managedMode && token === managedToken && !managedClient.closed) return managedSnapshot();
  await stopManagedCapture();
  managedMode = true; managedToken = token; managedState = null;
  managedClient.connect(token);
  return managedSnapshot();
}
async function managedCommand(action) {
  const state = managedState?.snapshot;
  if (!state || !managedClient.authenticated) throw new Error('control-not-ready');
  const result = await managedClient.send(createManagedCommand(state, crypto.randomUUID(), action));
  if (result.status !== 'ok') throw new Error(result.code || 'command-failed');
  return { ...result, state: managedState?.snapshot };
}
async function stopManagedCapture() {
  const pending = pendingCapture;
  await disposeManagedAudio();
  if (pending) await pending.catch(() => {});
  await disposeManagedAudio();
}
async function disposeManagedAudio() {
  // Close the producer even when media cleanup fails; do not close the controller.
  try { await stopCapture(); } finally { await disconnectSocket('capture stopped'); }
}
async function startManagedCapture({ leaseId, instanceId, generation, streamId, tabId }) {
  if (!managedMode || !managedClient.authenticated || managedState?.snapshot?.instanceId !== instanceId || managedState.snapshot.generation !== generation || managedState.snapshot.source !== 'chrome') throw new Error('stale-capture');
  await disposeManagedAudio();
  const connectionGeneration = ++socketGeneration;
  const producer = new WebSocket(AUDIO_URL); socket = producer;
  await new Promise((resolve, reject) => {
    const timeout = setTimeout(() => { producer.close(); reject(new Error('audio-connect-timeout')); }, 5000);
    const fail = () => { clearTimeout(timeout); reject(new Error('audio-disconnected')); };
    producer.addEventListener('open', () => {
      if (socket !== producer || socketGeneration !== connectionGeneration) { producer.close(); fail(); return; }
      producer.send(JSON.stringify({ type: 'hello', version: 1, token: managedToken, instanceId, leaseId, audio: AUDIO_FORMAT }));
    });
    producer.addEventListener('message', ({ data }) => {
      if (socket !== producer || socketGeneration !== connectionGeneration) return;
      let event; try { event = JSON.parse(data); } catch { producer.close(); fail(); return; }
      if (event.type === 'hello-ok' && event.version === 1 && event.instanceId === instanceId && event.generation === generation) { clearTimeout(timeout); authenticated = true; resolve(); }
      else { producer.close(); fail(); }
    });
    producer.addEventListener('close', () => {
      fail();
      if (socket === producer) { socket = null; authenticated = false; void stopCapture().catch(() => {}); broadcastManaged({ type: 'producer-disconnected' }); }
    });
    producer.addEventListener('error', () => { producer.close(); fail(); });
  });
  if (socket !== producer || socketGeneration !== connectionGeneration) throw new Error('capture-cancelled');
  await startCapture(streamId, tabId);
}

function publicError(error) {
  return error instanceof Error ? error.message : 'Unexpected offscreen error';
}

import { BRIDGE_URL, createCommand, createHello, parseServerEvent } from './shared/protocol.js';
import { PcmPacketizer, audioBacklogAction } from './shared/pcm.js';
import { GenerationGate } from './shared/generation-gate.js';

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

chrome.runtime.onMessage.addListener((message, _sender, sendResponse) => {
  if (!message || typeof message !== 'object') return undefined;

  let operation;
  switch (message.type) {
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
      operation = stopSession();
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
  if (typeof token !== 'string' || token.length === 0) throw new Error('A pairing token is required');
  const url = validateBridgeUrl(requestedUrl);
  await disconnectSocket();
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

function sendBrowserCommand(commandType, requestId, fields = {}) {
  if (!authenticated || socket?.readyState !== WebSocket.OPEN) {
    throw new Error('The local Sonos app is not paired');
  }
  socket.send(JSON.stringify(createCommand(commandType, requestId, fields)));
}

async function stopSession() {
  await stopCapture();
  await disconnectSocket();
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
  if (mediaStream) mediaStream.getTracks().forEach((track) => track.stop());
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

async function disconnectSocket() {
  authenticated = false;
  socketGeneration += 1;
  const activeSocket = socket;
  socket = null;
  if (activeSocket && activeSocket.readyState < WebSocket.CLOSING) activeSocket.close(1000, 'session ended');
}

function validateBridgeUrl(value) {
  const url = new URL(value ?? BRIDGE_URL);
  if (url.protocol !== 'ws:' || url.hostname !== '127.0.0.1' || url.pathname !== '/browser/v1') {
    throw new Error('Bridge URL must use ws://127.0.0.1:<port>/browser/v1');
  }
  return url.href;
}

function broadcast(event) {
  void chrome.runtime.sendMessage({ type: 'offscreen-event', event }).catch(() => {});
}

function publicError(error) {
  return error instanceof Error ? error.message : 'Unexpected offscreen error';
}

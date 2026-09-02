import { BRIDGE_URL, createCommand, createHello, parseServerEvent } from './shared/protocol.js';
import { PcmPacketizer, shouldDropAudio } from './shared/pcm.js';

const packetizer = new PcmPacketizer(960);

let socket = null;
let socketGeneration = 0;
let authenticated = false;
let mediaStream = null;
let audioContext = null;
let mediaSource = null;
let workletNode = null;
let backpressureWarningSent = false;
let endingTrack = null;

chrome.runtime.onMessage.addListener((message, _sender, sendResponse) => {
  if (!message || typeof message !== 'object') return undefined;

  let operation;
  switch (message.type) {
    case 'bridge-connect':
      operation = connectBridge(message.token, message.url);
      break;
    case 'start-capture':
      operation = startCapture(message.streamId);
      break;
    case 'stop-session':
      operation = stopSession();
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

async function startCapture(streamId) {
  if (!authenticated || socket?.readyState !== WebSocket.OPEN) {
    throw new Error('Pair with the local Sonos app before capturing audio');
  }
  if (typeof streamId !== 'string' || streamId.length === 0) {
    throw new Error('A tab capture stream ID is required');
  }

  await stopCapture();
  const stream = await navigator.mediaDevices.getUserMedia({
    audio: {
      mandatory: {
        chromeMediaSource: 'tab',
        chromeMediaSourceId: streamId,
      },
    },
    video: false,
  });

  let context = null;
  try {
    context = new AudioContext({ sampleRate: 48000 });
    await context.audioWorklet.addModule('audio-worklet.js');
    const source = context.createMediaStreamSource(stream);
    const node = new AudioWorkletNode(context, 'tab-audio-processor', {
      numberOfInputs: 1,
      numberOfOutputs: 0,
      channelCount: 2,
      channelCountMode: 'explicit',
      channelInterpretation: 'speakers',
    });
    node.port.onmessage = handleAudioQuantum;
    source.connect(node);
    await context.resume();

    mediaStream = stream;
    audioContext = context;
    mediaSource = source;
    workletNode = node;
    endingTrack = stream.getAudioTracks()[0] ?? null;
    if (endingTrack) endingTrack.onended = () => broadcast({ type: 'capture-ended' });
    packetizer.reset();
    backpressureWarningSent = false;
    broadcast({ type: 'capture-started' });
  } catch (error) {
    stream.getTracks().forEach((track) => track.stop());
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
    if (shouldDropAudio(activeSocket.bufferedAmount, packet.byteLength)) {
      if (!backpressureWarningSent) {
        backpressureWarningSent = true;
        broadcast({
          type: 'warning',
          code: 'audio-backpressure',
          message: 'Network backlog is high; live audio packets are being dropped',
        });
      }
      continue;
    }
    backpressureWarningSent = false;
    activeSocket.send(packet);
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

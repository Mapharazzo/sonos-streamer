const BYTES_PER_PIXEL = 4;
const MAX_WIDTH = 1920;
const MAX_HEIGHT = 1080;
const MIN_WIDTH = 640;
const MIN_HEIGHT = 360;

export function chooseFramePlan({ sourceWidth, sourceHeight, fps, delayMs, maxBytes }) {
  if (![sourceWidth, sourceHeight, fps, delayMs, maxBytes].every(Number.isFinite)) return null;
  if (sourceWidth <= 0 || sourceHeight <= 0 || fps <= 0 || maxBytes <= 0) return null;

  const sourceScale = Math.min(1, MAX_WIDTH / sourceWidth, MAX_HEIGHT / sourceHeight);
  const cappedWidth = Math.floor(sourceWidth * sourceScale);
  const cappedHeight = Math.floor(sourceHeight * sourceScale);
  const frameCount = Math.ceil((fps * Math.max(0, delayMs)) / 1000) + 2;
  const uncappedBytes = cappedWidth * cappedHeight * BYTES_PER_PIXEL * frameCount;
  const memoryScale = uncappedBytes > maxBytes ? Math.sqrt(maxBytes / uncappedBytes) : 1;
  const width = Math.floor(cappedWidth * memoryScale);
  const height = Math.floor(cappedHeight * memoryScale);

  if (width < MIN_WIDTH || height < MIN_HEIGHT) return null;

  const bytesPerFrame = width * height * BYTES_PER_PIXEL;
  const estimatedBytes = bytesPerFrame * frameCount;
  if (!Number.isSafeInteger(estimatedBytes) || estimatedBytes > maxBytes) return null;

  return {
    width,
    height,
    fps,
    delayMs: Math.max(0, delayMs),
    frameCount,
    bytesPerFrame,
    estimatedBytes,
    scale: width / sourceWidth,
  };
}

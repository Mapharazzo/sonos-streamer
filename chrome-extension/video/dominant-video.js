export function selectDominantVideo(candidates, viewport) {
  const bounds = normalizeViewport(viewport);
  let selected = null;
  let selectedArea = 0;

  for (const video of candidates ?? []) {
    if (!isEligible(video)) continue;
    const rect = video.getBoundingClientRect?.();
    if (!rect) continue;
    const visibleWidth = Math.max(0, Math.min(rect.right, bounds.right) - Math.max(rect.left, bounds.left));
    const visibleHeight = Math.max(0, Math.min(rect.bottom, bounds.bottom) - Math.max(rect.top, bounds.top));
    const visibleArea = visibleWidth * visibleHeight;
    if (visibleArea > selectedArea) {
      selected = video;
      selectedArea = visibleArea;
    }
  }
  return selected;
}

function isEligible(video) {
  if (!video || video.paused || video.ended || video.hidden) return false;
  if (Number.isFinite(video.readyState) && video.readyState < 2) return false;
  if (Number.isFinite(video.videoWidth) && video.videoWidth <= 0) return false;
  if (Number.isFinite(video.videoHeight) && video.videoHeight <= 0) return false;
  const style = video.style;
  if (style?.display === 'none' || style?.visibility === 'hidden') return false;
  return true;
}

function normalizeViewport(viewport) {
  const left = Number.isFinite(viewport?.left) ? viewport.left : 0;
  const top = Number.isFinite(viewport?.top) ? viewport.top : 0;
  const width = Number.isFinite(viewport?.width) ? Math.max(0, viewport.width) : 0;
  const height = Number.isFinite(viewport?.height) ? Math.max(0, viewport.height) : 0;
  return { left, top, right: left + width, bottom: top + height };
}

export function shortcutStatus(commands) {
  const shortcut = commands.find((command) => command.name === 'cast-current-tab')?.shortcut ?? '';
  return { available: shortcut.length > 0, shortcut };
}

export async function reloadSelectedTab({ tabId, expectedOrigin, tabs, stopCapture, waitReady, restorePlayback, timeoutMs = 15000 }) {
  const abort = new AbortController(); let timer;
  const check = async () => {
    if (abort.signal.aborted) throw new Error('reload-timeout');
    const tab = await tabs.get(tabId);
    if (tab.id !== tabId || new URL(tab.url).origin !== expectedOrigin) throw new Error('tab-changed');
    if (abort.signal.aborted) throw new Error('reload-timeout');
  };
  const operation = async () => {
    await stopCapture(); await check();
    await tabs.reload(tabId); await waitReady(tabId, abort.signal); await check();
    try { await restorePlayback(tabId, abort.signal); } catch { /* Seek/live/autoplay failure leaves manual recovery. */ }
    await check();
    return { tabId, status: 'invoke-extension' };
  };
  try {
    return await Promise.race([operation(), new Promise((_, reject) => { timer = setTimeout(() => { abort.abort(); reject(new Error('reload-timeout')); }, timeoutMs); })]);
  } finally { clearTimeout(timer); abort.abort(); }
}

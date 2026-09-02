class TabAudioProcessor extends AudioWorkletProcessor {
  process(inputs) {
    const input = inputs[0];
    if (!input || input.length === 0 || input[0].length === 0) return true;

    const left = new Float32Array(input[0]);
    const right = new Float32Array(input[1] ?? input[0]);
    this.port.postMessage({ type: 'audio-quantum', left, right }, [left.buffer, right.buffer]);
    return true;
  }
}

registerProcessor('tab-audio-processor', TabAudioProcessor);

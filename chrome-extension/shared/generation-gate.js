export class GenerationGate {
  #generation = 0;

  begin() {
    this.#generation += 1;
    return this.#generation;
  }

  cancel() {
    this.#generation += 1;
  }

  isCurrent(generation) {
    return generation === this.#generation;
  }
}

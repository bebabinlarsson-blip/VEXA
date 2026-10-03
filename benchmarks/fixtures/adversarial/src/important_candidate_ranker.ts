// Misleading filename and duplicate symbol: this is intentionally not the deterministic ranker.
export function selectCandidate<T>(candidates: readonly T[]): T | undefined {
  return candidates[0];
}


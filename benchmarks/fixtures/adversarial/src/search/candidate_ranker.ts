export type Candidate = Readonly<{
  path: string;
  score: number;
}>;

export function selectCandidate(candidates: readonly Candidate[]): Candidate | undefined {
  return [...candidates].sort((left, right) => {
    if (left.score !== right.score) {
      return right.score - left.score;
    }
    return left.path.localeCompare(right.path);
  })[0];
}


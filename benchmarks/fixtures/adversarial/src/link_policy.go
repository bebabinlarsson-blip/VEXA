package src

import "path/filepath"

func CanonicalRepoPath(root string, candidate string) (string, bool) {
	canonicalRoot, err := filepath.Abs(root)
	if err != nil {
		return "", false
	}
	canonicalCandidate, err := filepath.Abs(candidate)
	if err != nil {
		return "", false
	}
	relative, err := filepath.Rel(canonicalRoot, canonicalCandidate)
	if err != nil || relative == ".." || len(relative) >= 3 && relative[:3] == ".."+string(filepath.Separator) {
		return "", false
	}
	return canonicalCandidate, true
}


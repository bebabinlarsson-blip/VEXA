package bench;

public final class RetryBudget {
    private final int maximumAttempts;
    private int attemptsUsed;

    public RetryBudget(int maximumAttempts) {
        this.maximumAttempts = Math.max(0, maximumAttempts);
    }

    public int remainingAttempts() {
        return Math.max(0, maximumAttempts - attemptsUsed);
    }

    public void recordAttempt() {
        attemptsUsed += 1;
    }
}


using System;

namespace VexaBench;

public sealed class TokenLease
{
    public TokenLease(DateTimeOffset expiresAt)
    {
        ExpiresAt = expiresAt;
    }

    public DateTimeOffset ExpiresAt { get; }

    public bool IsExpired(DateTimeOffset now)
    {
        return now >= ExpiresAt;
    }
}


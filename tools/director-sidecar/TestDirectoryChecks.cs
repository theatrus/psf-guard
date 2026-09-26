internal static class TestDirectoryChecks
{
    internal static async Task RunAsync()
    {
        var primary = new InvalidOperationException("injected operation failure");
        string? cleaned = null;
        try
        {
            await TestDirectory.RunAsync("psf-guard-director-cleanup-check-", directory =>
            {
                cleaned = directory.FullName;
                throw primary;
            });
            throw new InvalidOperationException("Operation failure was swallowed.");
        }
        catch (InvalidOperationException error) when (ReferenceEquals(error, primary)) { }
        Assert(cleaned is not null && !Directory.Exists(cleaned), "operation failure preserves its exception and removes its directory");
        if (!OperatingSystem.IsWindows()) return;

        FileStream? held = null;
        Task release = Task.CompletedTask;
        try
        {
            await TestDirectory.RunAsync("psf-guard-director-cleanup-check-", directory =>
            {
                cleaned = directory.FullName;
                held = new FileStream(Path.Combine(cleaned, "execution.sqlite"), FileMode.Create, FileAccess.ReadWrite, FileShare.None);
                release = Task.Run(async () => { await Task.Delay(200); held.Dispose(); });
                return Task.CompletedTask;
            });
            await release;
            Assert(!Directory.Exists(cleaned), "transient Windows file lock is retried");
        }
        finally { await release; held?.Dispose(); }

        foreach (var failAction in new[] { true, false })
        {
            DirectoryInfo? retained = null;
            held = null;
            try
            {
                try
                {
                    await TestDirectory.RunAsync("psf-guard-director-cleanup-check-", directory =>
                    {
                        retained = directory;
                        held = new FileStream(Path.Combine(directory.FullName, "execution.sqlite"), FileMode.Create, FileAccess.ReadWrite, FileShare.None);
                        if (failAction) throw primary;
                        return Task.CompletedTask;
                    }, TimeSpan.FromMilliseconds(150));
                    throw new InvalidOperationException("Persistent lock failure was swallowed.");
                }
                catch (AggregateException error) when (failAction)
                {
                    Assert(error.InnerExceptions.Count == 2 && ReferenceEquals(error.InnerExceptions[0], primary)
                        && error.InnerExceptions[1] is IOException, "persistent cleanup failure retains the original operation error");
                }
                catch (IOException) when (!failAction)
                {
                    Console.WriteLine("PASS persistent cleanup failure fails an otherwise successful check");
                }
            }
            finally
            {
                held?.Dispose();
                // This is the exact directory created for the deliberate failure above.
                retained?.Delete(recursive: true);
            }
        }
    }

    private static void Assert(bool condition, string message)
    {
        if (!condition) throw new InvalidOperationException(message);
        Console.WriteLine($"PASS {message}");
    }
}

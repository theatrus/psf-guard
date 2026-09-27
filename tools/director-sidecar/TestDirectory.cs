using System.Diagnostics;

internal static class TestDirectory
{
    internal static async Task RunAsync(string prefix, Func<DirectoryInfo, Task> action,
        TimeSpan? cleanupTimeout = null)
    {
        var timeout = cleanupTimeout ?? TimeSpan.FromSeconds(5);
        if (timeout <= TimeSpan.Zero) throw new ArgumentOutOfRangeException(nameof(cleanupTimeout));
        var directory = Directory.CreateTempSubdirectory(prefix);
        Exception? primary = null;
        try { await action(directory); }
        catch (Exception error) { primary = error; throw; }
        finally
        {
            try { await DeleteAsync(directory, timeout); }
            catch (Exception cleanup) when (primary is not null)
            {
                throw new AggregateException($"Sidecar check failed and its test directory could not be removed: {directory.FullName}",
                    primary, cleanup);
            }
        }
    }

    private static async Task DeleteAsync(DirectoryInfo directory, TimeSpan timeout)
    {
        // Only freshly created test directories reach this method. Persistent
        // cleanup failure must fail the check without hiding its original error.
        var clock = Stopwatch.StartNew();
        while (true)
        {
            try { directory.Delete(recursive: true); return; }
            catch (DirectoryNotFoundException) when (!Directory.Exists(directory.FullName)) { return; }
            catch (IOException error) when (OperatingSystem.IsWindows()
                && (error.HResult & 0xffff) is 32 or 33 or 145 && clock.Elapsed < timeout)
            {
                await Task.Delay(TimeSpan.FromMilliseconds(50));
            }
        }
    }
}

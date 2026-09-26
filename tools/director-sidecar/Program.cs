using System.Diagnostics;
using System.Text.Json.Nodes;

if (args.Length < 2)
{
    Console.Error.WriteLine("Usage: Director.Sidecar <runtime.exe> <fixture.json> [fixture.json ...]");
    return 2;
}

var started = new List<int>();
var count = 0;
foreach (var fixturePath in args.Skip(1))
{
    var fixture = JsonNode.Parse(File.ReadAllText(fixturePath))!.AsObject();
    await using var session = await RuntimeSession.StartAsync(args[0], "rig-1", started.Add);
    foreach (var test in fixture["cases"]!.AsArray())
    {
        var input = fixture["base"]!.DeepClone();
        Merge(input.AsObject(), test!["patch"]!.AsObject());
        var result = await session.SendAsync(new JsonObject { ["type"] = "evaluate", ["request"] = input });
        var response = result["response"]!.AsObject();
        var actual = new JsonObject { ["status"] = response["status"]!.DeepClone() };
        if (response["decision"] is { } decision) actual["decision"] = decision.DeepClone();
        if (response["code"] is { } code) actual["code"] = code.DeepClone();
        if (!JsonNode.DeepEquals(actual, test["expected"]))
            throw new InvalidOperationException($"{test["name"]}: wrong sidecar decision.");
        Console.WriteLine($"PASS sidecar: {test["name"]}");
        count++;
    }
    await session.SendAsync(new JsonObject { ["type"] = "ping" });
    await session.SendAsync(new JsonObject { ["type"] = "shutdown" });
    Assert(await session.WaitForExitAsync() == 0, "graceful shutdown");
}

for (var iteration = 0; iteration < 10; iteration++)
{
    await using var slowReader = await RuntimeSession.StartAsync(args[0], "rig-1", started.Add);
    await slowReader.SendAsync(new JsonObject { ["type"] = "shutdown" }, replyDelay: TimeSpan.FromMilliseconds(100));
    Assert(await slowReader.WaitForExitAsync() == 0, "shutdown reply survives a delayed pipe reader");
}

await MustFail(async () =>
{
    await using var rejected = await RuntimeSession.StartAsync(args[0], "rig-1", started.Add, engineVersion: "incompatible");
}, "version mismatch");
await MustFail(async () =>
{
    await using var rejected = await RuntimeSession.StartAsync(args[0], "rig-1", started.Add, parentOverride: uint.MaxValue);
}, "pipe server PID mismatch");

await using (var session = await RuntimeSession.StartAsync(args[0], "rig-1", started.Add))
{
    session.Disconnect();
    Assert(await session.WaitForExitAsync() == 0, "parent disconnect exits the child");
    Assert(!session.IsUsable, "disconnected session unusable");
}
await using (var session = await RuntimeSession.StartAsync(args[0], "rig-1", started.Add))
{
    await session.SendInvalidLengthAsync();
    Assert(await session.WaitForExitAsync() != 0, "oversized header without body rejected");
}
await using (var session = await RuntimeSession.StartAsync(args[0], "rig-1", started.Add))
{
    await session.TruncateAsync();
    Assert(await session.WaitForExitAsync() != 0, "truncated body rejected");
}
await using (var session = await RuntimeSession.StartAsync(args[0], "rig-1", started.Add))
{
    await session.SendAsync(new JsonObject { ["type"] = "ping" });
    await session.SendDuplicateAsync();
    Assert(await session.WaitForExitAsync() != 0, "duplicate request rejected without replay");
}
await using (var session = await RuntimeSession.StartAsync(args[0], "rig-1", started.Add))
{
    session.KillChild();
    await session.WaitForExitAsync();
    await MustFail(async () => await session.SendAsync(new JsonObject { ["type"] = "ping" }), "child crash invalidates session");
    Assert(!session.IsUsable, "crashed session unusable");
}
// A new session after a crash must negotiate again; it does not inherit IDs or responses.
await using (var session = await RuntimeSession.StartAsync(args[0], "rig-1", started.Add))
{
    await session.SendAsync(new JsonObject { ["type"] = "ping" });
    using var cancelled = new CancellationTokenSource();
    cancelled.Cancel();
    await MustFail(async () => await session.SendAsync(new JsonObject { ["type"] = "ping" }, cancelled.Token), "cancellation before dispatch");
    Assert(session.IsUsable, "pre-dispatch cancellation leaves session intact");
    await session.SendAsync(new JsonObject { ["type"] = "shutdown" });
    Assert(await session.WaitForExitAsync() == 0, "restart handshake and shutdown");
}
await StorageChecks.RunAsync(args[0], args[1], started.Add);

foreach (var pid in started)
{
    try
    {
        using var process = Process.GetProcessById(pid);
        Assert(process.HasExited, "no orphan runtime processes");
    }
    catch (ArgumentException) { /* Exited process no longer has a PID entry. */ }
}
Console.WriteLine($"Passed {count} golden decisions and process lifecycle checks across {started.Count} owned sidecars.");
return 0;

static void Assert(bool condition, string label)
{
    if (!condition) throw new InvalidOperationException(label);
    Console.WriteLine($"PASS {label}");
}

static async Task MustFail(Func<Task> action, string label)
{
    try { await action(); }
    catch (Exception error) when (error is IOException or OperationCanceledException)
    {
        Console.WriteLine($"PASS {label}");
        return;
    }
    throw new InvalidOperationException($"Expected failure: {label}");
}

static void Merge(JsonObject target, JsonObject patch)
{
    foreach (var (key, value) in patch)
    {
        if (value is JsonObject nested && target[key] is JsonObject existing) Merge(existing, nested);
        else target[key] = value?.DeepClone();
    }
}

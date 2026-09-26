using System.Text.Json.Nodes;

// Real process/pipe recovery checks. No NINA or equipment dispatch is performed.
internal static class StorageChecks
{
    internal static async Task RunAsync(string executable, string fixturePath, Action<int> started)
    {
        var request = JsonNode.Parse(File.ReadAllText(fixturePath))!["base"]!.AsObject().DeepClone().AsObject();
        request["assignment"]!["revision"] = 9007199254740993UL;
        var goals = request["assignment"]!["goals"]!.AsArray();
        while (goals.Count > 1) goals.RemoveAt(goals.Count - 1);
        goals[0]!["requested"] = 1;
        goals[0]!["accepted"] = 0;
        goals[0]!["pending"] = 0;
        goals[0]!["attempts_remaining"] = 2;
        var state = request["state"]!;
        var directory = Directory.CreateTempSubdirectory("psf-guard-director-ledger-");
        try
        {
            await MustFail(async () =>
            {
                await using var old = await RuntimeSession.StartAsync(executable, "rig-1", started, runtimeVersion: "0.1.0");
            }, "legacy runtime handshake is rejected");
            await MustFail(async () =>
            {
                await using var invalid = await RuntimeSession.StartAsync(executable, "rig-1", started, storageDirectory: ":memory:");
            }, "ephemeral storage directory is rejected");
            await using (var disabled = await RuntimeSession.StartAsync(executable, "rig-1", started))
            {
                var response = await Send(disabled, Open(request));
                Assert(response["code"]!.GetValue<string>() == "disabled", "storage requires explicit launcher opt-in");
                await disabled.SendAsync(new JsonObject { ["type"] = "shutdown" });
                Assert(await disabled.WaitForExitAsync() == 0, "disabled storage leaves protocol usable");
            }

            string ledgerId;
            await using (var owner = await RuntimeSession.StartAsync(executable, "rig-1", started, storageDirectory: directory.FullName))
            {
                var opened = await Send(owner, Open(request));
                ledgerId = opened["info"]!["ledger_id"]!.GetValue<string>();
                Assert(opened["info"]!["assignment_revision"]!.GetValue<ulong>() == 9007199254740993UL,
                    "ledger assignment revision survives JSON without double rounding");
                await MustFail(async () =>
                {
                    await using var competing = await RuntimeSession.StartAsync(executable, "rig-1", started, storageDirectory: directory.FullName);
                }, "another live sidecar cannot own this directory");
                Assert(owner.IsUsable, "competing owner does not disrupt original owner");
                Assert(Outcome(await Send(owner, Reserve("capture-1", state))) == "created", "first capture is durably reserved");
                var uncertain = new JsonObject { ["state"] = "uncertain", ["reason"] = "save_timeout" };
                await Send(owner, Record(uncertain));
                owner.KillChild();
                Assert(await owner.WaitForExitAsync() != 0, "terminate child after committed uncertain outcome");
            }

            await using (var recovered = await RuntimeSession.StartAsync(executable, "rig-1", started, storageDirectory: directory.FullName))
            {
                Assert((await Send(recovered, Open(request)))["info"]!["ledger_id"]!.GetValue<string>() == ledgerId,
                    "restart releases OS ownership and preserves ledger identity");
                Assert(Outcome(await Send(recovered, Reserve("capture-1", state))) == "existing", "restart never replays prior capture");
                Assert(Outcome(await Send(recovered, Reserve("capture-2", state))) == "recovery_required", "uncertain capture blocks new work");
                var attempt = await Send(recovered, new JsonObject { ["action"] = "attempt", ["capture_id"] = "capture-1" });
                Assert(attempt["attempt"]!["evidence"]!["state"]!.GetValue<string>() == "uncertain", "recovery reads uncertain evidence");
                var saved = new JsonObject { ["state"] = "saved", ["image_id"] = "image-1", ["elapsed_ms"] = 1234UL };
                await Send(recovered, Record(saved));
                await Send(recovered, Record(saved));
                var first = await Send(recovered, new JsonObject { ["action"] = "events", ["after"] = 0, ["limit"] = 2 });
                Assert(first["events"]!.AsArray().Count == 2 && first["next_cursor"]!.GetValue<ulong>() == 2, "event page cursor is bounded and ordered");
                var page = new JsonObject { ["action"] = "events", ["after"] = 2, ["limit"] = 2 };
                var last = await Send(recovered, page);
                Assert(last["events"]!.AsArray().Count == 1 && last["next_cursor"]!.GetValue<ulong>() == 3, "duplicate save receipt adds no event");
                Assert(JsonNode.DeepEquals(last, await Send(recovered, page)), "event replay is stable");
                var conflict = await Send(recovered, Record(new JsonObject { ["state"] = "failed", ["reason"] = "late_failure" }));
                Assert(conflict["code"]!.GetValue<string>() == "conflicting_evidence", "terminal evidence cannot be overwritten");
                recovered.KillChild();
                await recovered.WaitForExitAsync();
            }

            await using (var saved = await RuntimeSession.StartAsync(executable, "rig-1", started, storageDirectory: directory.FullName))
            {
                var otherRevision = request.DeepClone().AsObject();
                otherRevision["assignment"]!["revision"] = 9007199254740994UL;
                var rejected = await Send(saved, Open(otherRevision));
                Assert(rejected["code"]!.GetValue<string>() == "assignment_mismatch", "new revision cannot reset ledger baseline");
                await Send(saved, Open(request));
                var pending = await Send(saved, Reserve("capture-2", state));
                Assert(Outcome(pending) == "decision" && pending["outcome"]!["value"]!["reason"]!.GetValue<string>() == "pending_assessment",
                    "saved image survives crash as pending rather than accepted or retryable");
                await MustFail(async () => await saved.SendAsync(new JsonObject { ["type"] = "evaluate", ["request"] = request.DeepClone() }),
                    "untracked evaluation cannot bypass opened ledger progress");
            }
        }
        finally
        {
            // This directory was created by this test, never supplied by a user.
            directory.Delete(recursive: true);
        }
    }

    private static JsonObject Open(JsonObject request) => new() { ["action"] = "open", ["request"] = request.DeepClone() };
    private static JsonObject Reserve(string id, JsonNode state) => new() { ["action"] = "reserve", ["capture_id"] = id, ["state"] = state.DeepClone() };
    private static JsonObject Record(JsonObject evidence) => new() { ["action"] = "record", ["capture_id"] = "capture-1", ["evidence"] = evidence.DeepClone() };
    private static string Outcome(JsonObject response) => response["outcome"]!["status"]!.GetValue<string>();
    private static async Task<JsonObject> Send(RuntimeSession session, JsonObject operation) =>
        (await session.SendAsync(new JsonObject { ["type"] = "ledger", ["operation"] = operation.DeepClone() }))["response"]!.AsObject();
    private static void Assert(bool condition, string label)
    {
        if (!condition) throw new InvalidOperationException(label);
        Console.WriteLine($"PASS {label}");
    }
    private static async Task MustFail(Func<Task> action, string label)
    {
        try { await action(); }
        catch (Exception error) when (error is IOException or OperationCanceledException)
        {
            Console.WriteLine($"PASS {label}");
            return;
        }
        throw new InvalidOperationException($"Expected failure: {label}");
    }
}

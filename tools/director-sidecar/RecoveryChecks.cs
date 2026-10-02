using System.Text.Json.Nodes;

// Exercises the real Windows sidecar and pipe, not NINA or device operations.
internal static class RecoveryChecks
{
    internal static async Task RunAsync(string executable, string fixturePath, Action<int> started)
    {
        await TestDirectory.RunAsync("psf-guard-director-recovery-", async root =>
        {
            var execution = root.CreateSubdirectory("allocation-one").FullName;
            var successor = root.CreateSubdirectory("allocation-two").FullName;
            var recovery = root.CreateSubdirectory("rig-session").FullName;
            var probe = Apply(2, 10102, new JsonObject { ["event"] = "begin_recovery", ["attempt_id"] = "probe" });
            var park = Apply(4, 10153, new JsonObject { ["event"] = "begin_park", ["attempt_id"] = "park" });
            await using (var owner = await RuntimeSession.StartAsync(executable, "rig-1", started,
                storageDirectory: execution, recoveryDirectory: recovery))
            {
                var open = JsonNode.Parse("""
                    {"action":"open","identity":{"rig_id":"rig-1","configuration_id":"config-1","night_id":"night-1","starts_at_ms":1000,"ends_at_ms":100000},
                     "policy":{"revision":9007199254740993,"quality_mode":"pause","bad_samples":2,"good_probes":2,"cooldown_ms":100,"maximum_hold_ms":1000,"maximum_probes":3,
                     "operation_timeout_ms":50,"evidence_max_age_ms":100,"latest_resume_ms":99000,"maximum_consecutive_failures":2,"maximum_total_failures":3,"park_on_stop":true},"now_ms":10000}
                    """)!.AsObject();
                var admitted = await Send(owner, open);
                Assert(admitted["created"]!.GetValue<bool>(), "recovery night admitted once");
                Assert(admitted["record"]!["snapshot"]!["policy"]!["revision"]!.GetValue<ulong>() == 9007199254740993UL,
                    "recovery policy revision stays exact across IPC");
                await MustFail(async () =>
                {
                    await using var competing = await RuntimeSession.StartAsync(executable, "rig-1", started,
                        storageDirectory: successor, recoveryDirectory: recovery);
                }, "per-rig recovery lease excludes another allocation process");
                Assert(owner.IsUsable, "recovery contention leaves owner intact");
                await Send(owner, Apply(0, 10001, Poor(10001)));
                await Send(owner, Apply(1, 10002, Poor(10002)));
                var issued = (await Send(owner, probe))["issued"]!;
                Assert(issued["operation"]!.GetValue<string>() == "probe" && issued["deadline_ms"]!.GetValue<ulong>() == 10152,
                    "probe gets an exact persisted identity and deadline");
                owner.KillChild();
                Assert(await owner.WaitForExitAsync() != 0, "kill sidecar after probe issuance");
            }
            await using (var restored = await RuntimeSession.StartAsync(executable, "rig-1", started,
                storageDirectory: execution, recoveryDirectory: recovery))
            {
                var replay = await Send(restored, probe);
                Assert(!replay["newly_applied"]!.GetValue<bool>() && replay.ContainsKey("issued") && replay["issued"] is null,
                    "lost probe reply cannot authorize redispatch after process restart");
                Assert(replay["record"]!["snapshot"]!["probes_spent"]!.GetValue<int>() == 1, "probe budget survives process restart");
                await Send(restored, Apply(3, 10152, new JsonObject { ["event"] = "tick" }));
                Assert((await Send(restored, park))["issued"]!["operation"]!.GetValue<string>() == "park", "park issued once after probe timeout");
                restored.KillChild();
                await restored.WaitForExitAsync();
            }
            await using (var restored = await RuntimeSession.StartAsync(executable, "rig-1", started,
                storageDirectory: successor, recoveryDirectory: recovery))
            {
                var replay = await Send(restored, park);
                Assert(!replay["newly_applied"]!.GetValue<bool>() && replay["issued"] is null, "successor allocation cannot reissue park");
                var stopped = await Send(restored, Apply(5, 10202, new JsonObject { ["event"] = "tick" }));
                Assert(stopped["record"]!["snapshot"]!["phase"]!["shutdown"]!.GetValue<string>() == "park_uncertain", "unverified park remains uncertain");
                var request = JsonNode.Parse(File.ReadAllText(fixturePath))!["base"]!.DeepClone();
                await restored.SendAsync(new JsonObject { ["type"] = "ledger", ["operation"] = new JsonObject { ["action"] = "open", ["request"] = request } });
                var state = request!["state"]!.DeepClone();
                state["now_ms"] = 10202;
                var blocked = await restored.SendAsync(new JsonObject { ["type"] = "ledger", ["operation"] = new JsonObject { ["action"] = "reserve", ["capture_id"] = "must-not-capture", ["state"] = state } });
                Assert(blocked["response"]!["status"]!.GetValue<string>() == "recovery_blocked", "night stop blocks a fresh allocation ledger");
                var events = await Send(restored, new JsonObject { ["action"] = "events", ["night_id"] = "night-1", ["after"] = 0, ["limit"] = 16 });
                Assert(events["next_cursor"]!.GetValue<ulong>() == 6 && events["events"]!.AsArray().Count == 6, "batch evidence contains no replay duplicates");
                await restored.SendAsync(new JsonObject { ["type"] = "shutdown" });
                Assert(await restored.WaitForExitAsync() == 0, "recovery sidecar closes cleanly");
            }
            await MustFail(async () =>
            {
                await using var invalid = await RuntimeSession.StartAsync(executable, "rig-1", started,
                    storageDirectory: execution, recoveryDirectory: execution);
            }, "recovery cannot use the allocation directory");
        });
    }

    private static JsonObject Apply(ulong revision, ulong now, JsonObject evidence) => new()
    {
        ["action"] = "apply",
        ["request"] = new JsonObject
        {
            ["night_id"] = "night-1",
            ["configuration_id"] = "config-1",
            ["event_id"] = $"event-{revision}",
            ["expected_revision"] = revision,
            ["now_ms"] = now,
            ["conditions"] = new JsonObject { ["safety"] = "safe", ["motion"] = "permitted" },
            ["event"] = evidence
        }
    };

    private static JsonObject Poor(ulong now) => new()
    {
        ["event"] = "quality",
        ["sample"] = new JsonObject
        {
            ["rig_id"] = "rig-1",
            ["configuration_id"] = "config-1",
            ["capture_id"] = $"capture-{now}",
            ["observed_at_ms"] = now,
            ["context"] = new JsonObject { ["target_id"] = "target", ["filter_id"] = "L", ["exposure_ms"] = 1000, ["bin_x"] = 1, ["bin_y"] = 1, ["reference_id"] = "reference", ["source"] = "pixels", ["algorithm_revision"] = "1" },
            ["verdict"] = "corroborated_poor"
        }
    };

    private static async Task<JsonObject> Send(RuntimeSession session, JsonObject operation) =>
        (await session.SendAsync(new JsonObject { ["type"] = "recovery", ["operation"] = new JsonObject { ["recovery_version"] = 1, ["operation"] = operation.DeepClone() } }))["response"]!.AsObject();

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

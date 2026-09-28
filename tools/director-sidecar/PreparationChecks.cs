using System.Text.Json.Nodes;

// The real owned-process host proves IPC recovery; this performs no device work.
internal static class PreparationChecks
{
    internal static async Task RunAsync(string executable, string fixturePath, Action<int> started)
    {
        var request = JsonNode.Parse(File.ReadAllText(fixturePath))!["base"]!.DeepClone().AsObject();
        request["assignment"]!["revision"] = 9007199254740993UL;
        var goals = request["assignment"]!["goals"]!.AsArray();
        while (goals.Count > 1) goals.RemoveAt(goals.Count - 1);
        goals[0]!["requested"] = 1;
        goals[0]!["accepted"] = 0;
        goals[0]!["pending"] = 0;
        goals[0]!["attempts_remaining"] = 2;
        var state = request["state"]!;
        JsonObject pending;
        await TestDirectory.RunAsync("psf-guard-director-preparation-", async directory =>
        {
            await using (var session = await RuntimeSession.StartAsync(executable, "rig-1", started, storageDirectory: directory.FullName))
            {
                await Send(session, new JsonObject { ["action"] = "open", ["request"] = request.DeepClone() });
                var selected = await Send(session, WithState("evaluate", state));
                Assert(selected["decision"]!["action"]!.GetValue<string>() == "acquire", "ledger selects the preparation goal without reserving an exposure");
                var empty = await Send(session, Page("events"));
                Assert(empty["events"]!.AsArray().Count == 0, "read-only planning consumes no capture attempt");
                var begin = new JsonObject
                {
                    ["action"] = "begin_preparation",
                    ["preparation_id"] = "prep-1",
                    ["context"] = new JsonObject
                    {
                        ["goal_id"] = selected["decision"]!["goal_id"]!.DeepClone(),
                        ["target_id"] = "target-1",
                        ["recipe_id"] = "recipe-1",
                        ["previous_target_id"] = "target-1",
                        ["filter_id"] = "ha",
                        ["readout_mode"] = 1,
                        ["mount_parked"] = false,
                        ["rotator_connected"] = false,
                        ["enable_slew_center"] = true,
                        ["dither_every"] = 0,
                        ["dither_override"] = null,
                        ["filter_exposures_since_dither"] = 0
                    },
                    ["estimates"] = new JsonObject
                    {
                        ["unpark_ms"] = 0,
                        ["center_ms"] = 0,
                        ["before_target_ms"] = 0,
                        ["dither_ms"] = 0,
                        ["filter_ms"] = 0,
                        ["readout_ms"] = 0,
                        ["capture_overhead_ms"] = 0
                    },
                    ["state"] = state.DeepClone()
                };
                Assert((await Send(session, begin))["created"]!.GetValue<bool>(), "preparation is durably started");
                var issued = await Send(session, Advance(state));
                Assert(issued["next"]!["status"]!.GetValue<string>() == "run", "one native operation is issued");
                pending = issued["next"]!["value"]!.DeepClone().AsObject();
                Assert(pending["operation"]!["operation"]!.GetValue<string>() == "switch_filter", "shared core chooses native preparation order");
                session.KillChild();
                Assert(await session.WaitForExitAsync() != 0, "terminate sidecar after issued native operation");
            }

            await using (var recovered = await RuntimeSession.StartAsync(executable, "rig-1", started, storageDirectory: directory.FullName))
            {
                await Send(recovered, new JsonObject { ["action"] = "open", ["request"] = request.DeepClone() });
                var active = await Send(recovered, new JsonObject { ["action"] = "active_preparation" });
                Assert(active["record"]!["preparation_id"]!.GetValue<string>() == "prep-1", "restart discovers preparation without a remembered ID");
                Assert(JsonNode.DeepEquals(active["record"]!["pending"], pending), "restart retains exact issued operation identity");
                var blocked = await Send(recovered, Advance(state));
                Assert(blocked["next"]!["status"]!.GetValue<string>() == "in_flight", "restart never reissues the operation");
                var close = await Send(recovered, new JsonObject { ["action"] = "close_preparation", ["preparation_id"] = "prep-1" });
                Assert(close["code"]!.GetValue<string>() == "conflicting_evidence", "close cannot discard an unresolved operation");
                var receipt = Complete(pending, state);
                await Send(recovered, receipt);
                var duplicate = await Send(recovered, receipt);
                Assert(duplicate["record"]!["observations"]!.AsArray().Count == 1, "native completion retry is idempotent");
                Assert(duplicate["record"]!["observations"]![0]!["completion"]!["elapsed_ms"]!.GetValue<ulong>() == 9007199254740993UL,
                    "native operation timings remain exact across JSON");
                var next = await Send(recovered, Advance(state));
                var second = next["next"]!["value"]!.AsObject();
                Assert(second["ordinal"]!.GetValue<uint>() == 2, "receipt permits only the next operation");
                await Send(recovered, Complete(second, state));
                Assert((await Send(recovered, Advance(state)))["next"]!["status"]!.GetValue<string>() == "ready_to_reserve", "shared core rechecks final preparation boundary");
                var capture = WithState("reserve_prepared", state);
                capture["preparation_id"] = "prep-1";
                capture["capture_id"] = "capture-1";
                var reserved = await Send(recovered, capture);
                Assert(reserved["outcome"]!["status"]!.GetValue<string>() == "created", "prepared capture is reserved and linked atomically");
                recovered.KillChild();
                await recovered.WaitForExitAsync();
            }

            await using (var captured = await RuntimeSession.StartAsync(executable, "rig-1", started, storageDirectory: directory.FullName))
            {
                await Send(captured, new JsonObject { ["action"] = "open", ["request"] = request.DeepClone() });
                Assert((await Send(captured, new JsonObject { ["action"] = "active_preparation" }))["record"] is null,
                    "reserved preparation is no longer active");
                var unresolved = await Send(captured, new JsonObject { ["action"] = "unresolved_attempt" });
                Assert(unresolved["attempt"]!["capture_id"]!.GetValue<string>() == "capture-1", "restart discovers unresolved capture identity");
                var capture = WithState("reserve_prepared", state);
                capture["preparation_id"] = "prep-1";
                capture["capture_id"] = "capture-1";
                Assert((await Send(captured, capture))["outcome"]!["status"]!.GetValue<string>() == "existing", "capture retry cannot authorize redispatch");
                await Send(captured, new JsonObject
                {
                    ["action"] = "record",
                    ["capture_id"] = "capture-1",
                    ["evidence"] = new JsonObject { ["state"] = "saved", ["image_id"] = "image-1", ["elapsed_ms"] = 30000 }
                });
                Assert((await Send(captured, WithState("evaluate", state)))["decision"]!["reason"]!.GetValue<string>() == "pending_assessment",
                    "prepared capture remains pending grading, not accepted or retryable");
                var events = await Send(captured, Page("preparation_events"));
                Assert(events["events"]!.AsArray().Count == 6 && events["next_cursor"]!.GetValue<ulong>() == 6, "preparation outbox replays without duplicate receipts");
                await captured.SendAsync(new JsonObject { ["type"] = "shutdown" });
                Assert(await captured.WaitForExitAsync() == 0, "preparation recovery session shuts down cleanly");
            }
        });
    }

    private static JsonObject WithState(string action, JsonNode state) => new() { ["action"] = action, ["state"] = state.DeepClone() };
    private static JsonObject Advance(JsonNode state)
    {
        var command = WithState("advance_preparation", state);
        command["preparation_id"] = "prep-1";
        return command;
    }
    private static JsonObject Complete(JsonObject command, JsonNode state) => new()
    {
        ["action"] = "complete_preparation",
        ["completion"] = new JsonObject
        {
            ["preparation_id"] = command["preparation_id"]!.DeepClone(),
            ["ordinal"] = command["ordinal"]!.DeepClone(),
            ["ended_at_ms"] = state["now_ms"]!.DeepClone(),
            ["elapsed_ms"] = 9007199254740993UL,
            ["outcome"] = new JsonObject { ["outcome"] = "succeeded" }
        }
    };
    private static JsonObject Page(string action) => new() { ["action"] = action, ["after"] = 0, ["limit"] = 32 };
    private static async Task<JsonObject> Send(RuntimeSession session, JsonObject operation) =>
        (await session.SendAsync(new JsonObject { ["type"] = "ledger", ["operation"] = operation.DeepClone() }))["response"]!.AsObject();
    private static void Assert(bool condition, string label)
    {
        if (!condition) throw new InvalidOperationException(label);
        Console.WriteLine($"PASS {label}");
    }
}

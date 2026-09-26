using System.Text.Json.Nodes;

internal static class ProgramChecks
{
    internal static async Task RunAsync(string executable, string fixturePath, Action<int> started)
    {
        var program = JsonNode.Parse(File.ReadAllText(Path.Combine(Path.GetDirectoryName(fixturePath)!, "execution-program.json")))!.AsObject();
        var state = JsonNode.Parse(File.ReadAllText(fixturePath))!["base"]!["state"]!;
        JsonObject issued;
        JsonObject binding;
        await TestDirectory.RunAsync("psf-guard-director-program-", async directory =>
        {
            await using (var session = await RuntimeSession.StartAsync(executable, "rig-1", started, storageDirectory: directory.FullName))
            {
                var opened = await Send(session, Open(program, state));
                Assert(opened["status"]!.GetValue<string>() == "program_opened" && opened["program_version"]!.GetValue<int>() == 1, "program-bound storage is explicit");
                Assert(opened["info"]!["assignment_revision"]!.GetValue<ulong>() == 9007199254740993UL, "bound allocation revision is exact");
                var begin = new JsonObject
                {
                    ["action"] = "begin_program_preparation",
                    ["preparation_id"] = "prep",
                    ["goal_id"] = "short-ha",
                    ["local"] = new JsonObject
                    {
                        ["configuration"] = program["configuration"]!.DeepClone(),
                        ["previous_pointing"] = new JsonObject
                        {
                            ["configuration_id"] = program["configuration"]!["id"]!.DeepClone(),
                            ["target"] = program["targets"]![0]!.DeepClone()
                        },
                        ["mount_parked"] = false,
                        ["rotator_connected"] = false,
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
                Assert((await Send(session, begin))["created"]!.GetValue<bool>(), "program resolves and journals preparation");
                var response = await Send(session, Boundary("advance_program_preparation", program, state));
                Assert(response["next"]!["status"]!.GetValue<string>() == "run", "program issues one native operation");
                issued = response["next"]!["value"]!.DeepClone().AsObject();
                Assert(issued["operation"]!["operation"]!.GetValue<string>() == "switch_filter", "program preparation uses shared ordering");
                session.KillChild();
                Assert(await session.WaitForExitAsync() != 0, "kill bound sidecar after issued operation");
            }

            await using (var recovered = await RuntimeSession.StartAsync(executable, "rig-1", started, storageDirectory: directory.FullName))
            {
                var changed = program.DeepClone().AsObject();
                changed["recipes"]![0]!["gain"] = 41;
                Assert((await Send(recovered, Open(changed, state)))["code"]!.GetValue<string>() == "assignment_mismatch", "restart refuses a changed capture recipe");
                var legacy = new JsonObject
                {
                    ["action"] = "open",
                    ["request"] = new JsonObject { ["contract_version"] = 2, ["assignment"] = program["assignment"]!.DeepClone(), ["state"] = state.DeepClone() }
                };
                Assert((await Send(recovered, legacy))["code"]!.GetValue<string>() == "assignment_mismatch", "restart cannot drop program binding");
                await Send(recovered, Open(program, state));
                var active = await Send(recovered, new JsonObject { ["action"] = "active_preparation" });
                Assert(JsonNode.DeepEquals(active["record"]!["pending"], issued), "restart retains exact bound operation");
                Assert((await Send(recovered, Boundary("advance_program_preparation", program, state)))["next"]!["status"]!.GetValue<string>() == "in_flight", "bound operation is never reissued after restart");
                await Send(recovered, Complete(issued, state));
                var next = await Send(recovered, Boundary("advance_program_preparation", program, state));
                await Send(recovered, Complete(next["next"]!["value"]!.AsObject(), state));
                var stale = Boundary("reserve_program_prepared", program, state);
                stale["configuration"]!["filters"]![0]!["position"] = 3;
                Assert((await Send(recovered, stale))["code"]!.GetValue<string>() == "assignment_mismatch", "changed wheel slot prevents capture reservation");
                Assert((await Send(recovered, Boundary("reserve_program_prepared", program, state)))["outcome"]!["status"]!.GetValue<string>() == "created", "bound capture reserves once");
                binding = (await Send(recovered, Lookup()))["binding"]!.DeepClone().AsObject();
                Assert(JsonNode.DeepEquals(binding["target"], program["targets"]![0]) && JsonNode.DeepEquals(binding["recipe"], program["recipes"]![0]) && JsonNode.DeepEquals(binding["configuration"], program["configuration"]), "capture evidence resolves exact target recipe and equipment");
                recovered.KillChild();
                Assert(await recovered.WaitForExitAsync() != 0, "kill bound sidecar after reserved capture");
            }

            await using (var captured = await RuntimeSession.StartAsync(executable, "rig-1", started, storageDirectory: directory.FullName))
            {
                await Send(captured, Open(program, state));
                Assert(JsonNode.DeepEquals((await Send(captured, Lookup()))["binding"], binding), "capture binding survives process death unchanged");
                Assert((await Send(captured, Boundary("reserve_program_prepared", program, state)))["outcome"]!["status"]!.GetValue<string>() == "existing", "recovered capture evidence never grants redispatch");
                await Send(captured, new JsonObject
                {
                    ["action"] = "record",
                    ["capture_id"] = "capture",
                    ["evidence"] = new JsonObject { ["state"] = "saved", ["image_id"] = "image", ["elapsed_ms"] = 9007199254740993UL }
                });
                var saved = (await Send(captured, Lookup()))["binding"]!;
                Assert(saved["attempt"]!["evidence"]!["elapsed_ms"]!.GetValue<ulong>() == 9007199254740993UL, "capture timing remains exact in binding response");
                var evaluation = await Send(captured, new JsonObject { ["action"] = "evaluate", ["state"] = state.DeepClone() });
                Assert(evaluation["decision"]!["reason"]!.GetValue<string>() == "pending_assessment", "program capture waits for grading");
                await captured.SendAsync(new JsonObject { ["type"] = "shutdown" });
                Assert(await captured.WaitForExitAsync() == 0, "bound program recovery shuts down cleanly");
            }
        });
    }

    private static JsonObject Open(JsonObject program, JsonNode state) => new()
    {
        ["action"] = "open_program",
        ["program"] = program.DeepClone(),
        ["state"] = state.DeepClone()
    };
    private static JsonObject Boundary(string action, JsonObject program, JsonNode state)
    {
        var operation = new JsonObject
        {
            ["action"] = action,
            ["preparation_id"] = "prep",
            ["configuration"] = program["configuration"]!.DeepClone(),
            ["state"] = state.DeepClone()
        };
        if (action == "reserve_program_prepared") operation["capture_id"] = "capture";
        return operation;
    }
    private static JsonObject Lookup() => new() { ["action"] = "capture_binding", ["capture_id"] = "capture" };
    private static JsonObject Complete(JsonObject command, JsonNode state) => new()
    {
        ["action"] = "complete_preparation",
        ["completion"] = new JsonObject
        {
            ["preparation_id"] = command["preparation_id"]!.DeepClone(),
            ["ordinal"] = command["ordinal"]!.DeepClone(),
            ["ended_at_ms"] = state["now_ms"]!.DeepClone(),
            ["elapsed_ms"] = 1,
            ["outcome"] = new JsonObject { ["outcome"] = "succeeded" }
        }
    };
    private static async Task<JsonObject> Send(RuntimeSession session, JsonObject operation) =>
        (await session.SendAsync(new JsonObject { ["type"] = "ledger", ["operation"] = operation.DeepClone() }))["response"]!.AsObject();
    private static void Assert(bool condition, string label)
    {
        if (!condition) throw new InvalidOperationException(label);
        Console.WriteLine($"PASS {label}");
    }
}

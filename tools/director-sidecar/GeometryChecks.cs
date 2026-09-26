using System.Text.Json.Nodes;

internal static class GeometryChecks
{
    internal static async Task RunAsync(string executable, string fixturePath, Action<int> started)
    {
        var fixtures = Path.GetDirectoryName(fixturePath)!;
        var program = JsonNode.Parse(File.ReadAllText(Path.Combine(fixtures, "execution-program.json")))!.AsObject();
        var geometry = JsonNode.Parse(File.ReadAllText(Path.Combine(fixtures, "../../../director-runtime/tests/fixtures/geometry.json")))!;
        var state = geometry["state"]!;
        var constraints = geometry["constraints"]!;
        var start = state["now_ms"]!.GetValue<ulong>();
        program["assignment"]!["valid_from_ms"] = start;
        program["assignment"]!["expires_at_ms"] = start + 60000;
        var goal = program["assignment"]!["goals"]![0]!;
        goal["eligible_windows"] = new JsonArray(new JsonObject { ["start_ms"] = start, ["end_ms"] = start + 60000 });
        goal["exposure_ms"] = 5000;
        goal["overhead_ms"] = 1000;
        program["recipes"]![0]!["exposure_ms"] = 5000;
        var directory = Directory.CreateTempSubdirectory("psf-guard-director-geometry-");
        JsonNode issued;
        try
        {
            await using (var session = await RuntimeSession.StartAsync(executable, "rig-1", started, storageDirectory: directory.FullName))
            {
                var opened = await Send(session, Open(program, constraints, state));
                Assert(opened["status"]!.GetValue<string>() == "geometry_opened" && opened["constraints_version"]!.GetValue<int>() == 1, "geometry mode is explicit");
                var begin = new JsonObject
                {
                    ["action"] = "begin_geometry_preparation",
                    ["preparation_id"] = "prep",
                    ["goal_id"] = "short-ha",
                    ["state"] = state.DeepClone(),
                    ["constraints"] = constraints.DeepClone(),
                    ["local"] = new JsonObject
                    {
                        ["configuration"] = program["configuration"]!.DeepClone(),
                        ["previous_pointing"] = null,
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
                    }
                };
                Assert((await Send(session, begin))["created"]!.GetValue<bool>(), "geometry preparation committed");
                var next = (await Send(session, Boundary("advance_geometry_preparation", program, constraints, state)))["next"]!;
                Assert(next["status"]!.GetValue<string>() == "run", "geometry issues one preparation operation");
                issued = next["value"]!.DeepClone();
                session.KillChild();
                Assert(await session.WaitForExitAsync() != 0, "kill geometry sidecar after issue");
            }
            await using (var session = await RuntimeSession.StartAsync(executable, "rig-1", started, storageDirectory: directory.FullName))
            {
                var changed = constraints.DeepClone();
                changed["rig"]!["site"]!["latitude_degrees"] = 36.0;
                Assert((await Send(session, Open(program, changed, state)))["code"]!.GetValue<string>() == "assignment_mismatch", "geometry restart refuses changed content with identical IDs");
                Assert((await Send(session, new JsonObject { ["action"] = "open_program", ["program"] = program.DeepClone(), ["state"] = state.DeepClone() }))["code"]!.GetValue<string>() == "assignment_mismatch", "geometry cannot reopen without constraints");
                await Send(session, Open(program, constraints, state));
                var active = (await Send(session, new JsonObject { ["action"] = "active_preparation" }))["record"]!;
                Assert(JsonNode.DeepEquals(active["pending"], issued), "geometry restart discovers exact pending command");
                Assert((await Send(session, Boundary("advance_geometry_preparation", program, constraints, state)))["next"]!["status"]!.GetValue<string>() == "in_flight", "geometry restart never repeats hardware command");
                await Send(session, Complete(issued, state));
                for (var count = 0; ; count++)
                {
                    Assert(count < 10, "geometry preparation remains bounded");
                    var next = (await Send(session, Boundary("advance_geometry_preparation", program, constraints, state)))["next"]!;
                    if (next["status"]!.GetValue<string>() == "ready_to_reserve") break;
                    Assert(next["status"]!.GetValue<string>() == "run", "geometry preparation advances");
                    await Send(session, Complete(next["value"]!, state));
                }
                var legacy = Boundary("reserve_program_prepared", program, constraints, state);
                legacy.Remove("constraints");
                Assert((await Send(session, legacy))["code"]!.GetValue<string>() == "conflicting_evidence", "geometry rejects program-only reservation bypass");
                Assert((await Send(session, Boundary("reserve_geometry_prepared", program, constraints, state)))["outcome"]!["status"]!.GetValue<string>() == "created", "geometry reserves capture once");
                session.KillChild();
                Assert(await session.WaitForExitAsync() != 0, "kill geometry sidecar after capture reservation");
            }
            await using (var session = await RuntimeSession.StartAsync(executable, "rig-1", started, storageDirectory: directory.FullName))
            {
                await Send(session, Open(program, constraints, state));
                Assert((await Send(session, Boundary("reserve_geometry_prepared", program, constraints, state)))["outcome"]!["status"]!.GetValue<string>() == "existing", "geometry recovered capture never grants redispatch");
                var binding = (await Send(session, new JsonObject { ["action"] = "capture_binding", ["capture_id"] = "capture" }))["binding"]!;
                Assert(JsonNode.DeepEquals(binding["recipe"], program["recipes"]![0]), "geometry capture keeps exact recipe");
                await session.SendAsync(new JsonObject { ["type"] = "shutdown" });
                Assert(await session.WaitForExitAsync() == 0, "geometry recovery shuts down cleanly");
            }
        }
        finally
        {
            // Only this test-owned temporary directory is removed.
            directory.Delete(recursive: true);
        }
    }

    private static JsonObject Open(JsonNode program, JsonNode constraints, JsonNode state) => new()
    {
        ["action"] = "open_geometry",
        ["program"] = program.DeepClone(),
        ["constraints"] = constraints.DeepClone(),
        ["state"] = state.DeepClone()
    };
    private static JsonObject Boundary(string action, JsonNode program, JsonNode constraints, JsonNode state)
    {
        var operation = new JsonObject
        {
            ["action"] = action,
            ["preparation_id"] = "prep",
            ["configuration"] = program["configuration"]!.DeepClone(),
            ["constraints"] = constraints.DeepClone(),
            ["state"] = state.DeepClone()
        };
        if (action.StartsWith("reserve_", StringComparison.Ordinal)) operation["capture_id"] = "capture";
        return operation;
    }
    private static JsonObject Complete(JsonNode command, JsonNode state) => new()
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

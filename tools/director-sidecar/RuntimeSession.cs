using System.Buffers.Binary;
using System.Diagnostics;
using System.IO.Pipes;
using System.Runtime.InteropServices;
using System.Text.Json;
using System.Text.Json.Nodes;
using Microsoft.Win32.SafeHandles;

// Process-level test host, not a NINA plugin or production dispatch API.
internal sealed class RuntimeSession : IAsyncDisposable
{
    internal const int MaxFrameBytes = 262144 + 4096;
    internal const string EngineVersion = "0.2.0";
    internal const int ContractVersion = 2;
    private readonly NamedPipeServerStream pipe;
    private readonly Process process;
    private readonly SemaphoreSlim gate = new(1, 1);
    private readonly string sessionId = Guid.NewGuid().ToString("N");
    private ulong nextId = 1;
    private volatile bool ready;
    private bool disposed;

    private RuntimeSession(NamedPipeServerStream pipe, Process process)
    {
        this.pipe = pipe;
        this.process = process;
    }

    internal bool IsUsable => !disposed && ready && !process.HasExited;
    internal int ProcessId => process.Id;

    internal static async Task<RuntimeSession> StartAsync(string executable, string rigId,
        Action<int> started, string engineVersion = EngineVersion, uint? parentOverride = null)
    {
        var pipeName = $"psf-guard-director-{Guid.NewGuid():N}";
        var pipe = new NamedPipeServerStream(pipeName, PipeDirection.InOut, 1,
            PipeTransmissionMode.Byte, PipeOptions.Asynchronous | PipeOptions.CurrentUserOnly | PipeOptions.FirstPipeInstance);
        Process? process = null;
        RuntimeSession? session = null;
        try
        {
            var info = new ProcessStartInfo(Path.GetFullPath(executable))
            {
                UseShellExecute = false,
                CreateNoWindow = true,
                WorkingDirectory = Path.GetDirectoryName(Path.GetFullPath(executable))!
            };
            info.ArgumentList.Add("--pipe");
            info.ArgumentList.Add(pipeName);
            info.ArgumentList.Add("--parent-pid");
            info.ArgumentList.Add((parentOverride ?? (uint)Environment.ProcessId).ToString(System.Globalization.CultureInfo.InvariantCulture));
            process = Process.Start(info) ?? throw new IOException("Sidecar did not start.");
            started(process.Id);
            session = new RuntimeSession(pipe, process);
            using var startup = new CancellationTokenSource(TimeSpan.FromSeconds(15));
            var connection = pipe.WaitForConnectionAsync(startup.Token);
            var exit = process.WaitForExitAsync(startup.Token);
            if (await Task.WhenAny(connection, exit) == exit)
                throw new IOException("Sidecar exited before connection.");
            await connection;
            if (!GetNamedPipeClientProcessId(pipe.SafePipeHandle, out var peer) || peer != (uint)process.Id)
                throw new IOException("Control pipe peer is not the owned sidecar process.");

            var payload = new JsonObject
            {
                ["type"] = "hello",
                ["runtime_version"] = "0.1.1",
                ["engine_version"] = engineVersion,
                ["contract_version"] = ContractVersion,
                ["rig_id"] = rigId
            };
            var result = await session.ExchangeAsync(0, payload, "ready", startup.Token);
            if (result["runtime_version"]?.GetValue<string>() != "0.1.1" ||
                result["engine_version"]?.GetValue<string>() != EngineVersion ||
                result["contract_version"]?.GetValue<int>() != ContractVersion ||
                result["rig_id"]?.GetValue<string>() != rigId)
                throw new InvalidDataException("Sidecar handshake identity/version mismatch.");
            session.ready = true;
            return session;
        }
        catch
        {
            if (session is not null) await session.DisposeAsync();
            else
            {
                pipe.Dispose();
                if (process is not null)
                {
                    StopOwnedProcess(process);
                    await process.WaitForExitAsync();
                    process.Dispose();
                }
            }
            throw;
        }
    }

    internal async Task<JsonObject> SendAsync(JsonObject payload, CancellationToken token = default, TimeSpan? replyDelay = null)
    {
        // Cancellation before taking the gate cannot abandon an in-flight frame.
        await gate.WaitAsync(token);
        try
        {
            if (!IsUsable) throw new IOException("Sidecar session is no longer usable.");
            using var deadline = CancellationTokenSource.CreateLinkedTokenSource(token);
            deadline.CancelAfter(TimeSpan.FromSeconds(5));
            var expected = payload["type"]?.GetValue<string>() switch
            {
                "evaluate" => "decision",
                "ping" => "pong",
                "shutdown" => "stopped",
                _ => throw new InvalidDataException("Unsupported host request.")
            };
            var response = await ExchangeAsync(checked(nextId++), payload, expected, deadline.Token, replyDelay);
            if (expected == "stopped") { ready = false; pipe.Dispose(); }
            if (expected == "decision")
            {
                var decision = response["response"]!.AsObject();
                if (decision["contract_version"]?.GetValue<int>() != ContractVersion ||
                    decision["engine_version"]?.GetValue<string>() != EngineVersion)
                    throw new InvalidDataException("Wrong decision contract.");
                if (decision["status"]?.GetValue<string>() == "ok" &&
                    (decision["assignment_id"]?.GetValue<string>() != payload["request"]!["assignment"]!["id"]!.GetValue<string>() ||
                     decision["assignment_revision"]?.GetValue<ulong>() != payload["request"]!["assignment"]!["revision"]!.GetValue<ulong>()))
                    throw new InvalidDataException("Wrong decision assignment revision.");
            }
            return response;
        }
        catch
        {
            Abort();
            throw;
        }
        finally { gate.Release(); }
    }

    private async Task<JsonObject> ExchangeAsync(ulong id, JsonObject payload, string expected, CancellationToken token, TimeSpan? replyDelay = null)
    {
        var message = new JsonObject
        {
            ["protocol_version"] = 2,
            ["session_id"] = sessionId,
            ["request_id"] = id,
            ["payload"] = payload.DeepClone()
        };
        await WriteFrameAsync(pipe, JsonSerializer.SerializeToUtf8Bytes(message), token);
        if (replyDelay is { } delay) await Task.Delay(delay, token);
        var response = JsonNode.Parse(await ReadFrameAsync(pipe, token))!.AsObject();
        if (response.Count != 4 || response["protocol_version"]?.GetValue<int>() != 2 ||
            response["session_id"]?.GetValue<string>() != sessionId ||
            response["request_id"]?.GetValue<ulong>() != id ||
            response["payload"]?["type"]?.GetValue<string>() != expected)
            throw new InvalidDataException("Uncorrelated or incompatible sidecar response.");
        return response["payload"]!.AsObject();
    }

    private static async Task WriteFrameAsync(Stream stream, byte[] bytes, CancellationToken token)
    {
        if (bytes.Length == 0 || bytes.Length > MaxFrameBytes) throw new InvalidDataException("Invalid frame length.");
        byte[] header = new byte[4];
        BinaryPrimitives.WriteUInt32LittleEndian(header, (uint)bytes.Length);
        await stream.WriteAsync(header, token);
        await stream.WriteAsync(bytes, token);
        await stream.FlushAsync(token);
    }

    private static async Task<byte[]> ReadFrameAsync(Stream stream, CancellationToken token)
    {
        byte[] header = new byte[4];
        await stream.ReadExactlyAsync(header, token);
        var length = BinaryPrimitives.ReadUInt32LittleEndian(header);
        if (length == 0 || length > MaxFrameBytes) throw new InvalidDataException("Invalid reply frame length.");
        var body = new byte[checked((int)length)];
        await stream.ReadExactlyAsync(body, token);
        return body;
    }

    // Fault injection stays in this test harness, never in the planning protocol.
    internal async Task SendInvalidLengthAsync()
    {
        await pipe.WriteAsync(new byte[] { 255, 255, 255, 255 });
        await pipe.FlushAsync();
    }

    internal async Task SendDuplicateAsync()
    {
        var message = new JsonObject
        {
            ["protocol_version"] = 2,
            ["session_id"] = sessionId,
            ["request_id"] = nextId - 1,
            ["payload"] = new JsonObject { ["type"] = "ping" }
        };
        await WriteFrameAsync(pipe, JsonSerializer.SerializeToUtf8Bytes(message), CancellationToken.None);
    }

    internal async Task TruncateAsync()
    {
        await pipe.WriteAsync(new byte[] { 10, 0, 0, 0, 123 });
        await pipe.FlushAsync();
        pipe.Dispose();
        ready = false;
    }

    internal void Disconnect() { ready = false; pipe.Dispose(); }
    internal void KillChild() { process.Kill(entireProcessTree: true); }

    internal async Task<int> WaitForExitAsync()
    {
        using var timeout = new CancellationTokenSource(TimeSpan.FromSeconds(5));
        await process.WaitForExitAsync(timeout.Token);
        ready = false;
        return process.ExitCode;
    }

    private void Abort()
    {
        ready = false;
        pipe.Dispose();
        StopOwnedProcess(process);
    }

    private static void StopOwnedProcess(Process child)
    {
        try
        {
            if (!child.HasExited) child.Kill(entireProcessTree: true);
        }
        catch (InvalidOperationException) when (child.HasExited)
        {
            // The child can exit between checking HasExited and calling Kill.
        }
    }

    public async ValueTask DisposeAsync()
    {
        if (disposed) return;
        disposed = true;
        Abort();
        await process.WaitForExitAsync();
        process.Dispose();
    }

    [DllImport("kernel32.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool GetNamedPipeClientProcessId(SafePipeHandle pipe, out uint clientProcessId);
}

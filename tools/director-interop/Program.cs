using System.Runtime.InteropServices;
using System.Text.Json.Nodes;

if (args.Length != 2)
{
    Console.Error.WriteLine("Usage: Director.Interop <native-library-path> <decisions.json>");
    return 2;
}

var libraryPath = Path.GetFullPath(args[0]);
NativeLibrary.SetDllImportResolver(typeof(Native).Assembly, (name, assembly, path) =>
    name == "psf_guard_director_ffi" ? NativeLibrary.Load(libraryPath) : IntPtr.Zero);
if (Native.AbiVersion() != 1)
{
    throw new InvalidOperationException("Unsupported Director native ABI.");
}

var fixture = JsonNode.Parse(File.ReadAllText(args[1]))!.AsObject();
var cases = fixture["cases"]!.AsArray();
foreach (var test in cases)
{
    var input = fixture["base"]!.DeepClone();
    Merge(input.AsObject(), test!["patch"]!.AsObject());
    var bytes = System.Text.Encoding.UTF8.GetBytes(input.ToJsonString());
    var output = Native.Evaluate(bytes);
    var response = JsonNode.Parse(output)!.AsObject();
    if (response["contract_version"]!.GetValue<int>() != 1 || response["engine_version"]!.GetValue<string>() != "0.1.0")
    {
        throw new InvalidOperationException("Unexpected planning contract or engine version.");
    }
    var expected = test["expected"]!.AsObject();
    if (response["status"]!.GetValue<string>() == "ok" &&
        (response["assignment_id"]!.GetValue<string>() != input["assignment"]!["id"]!.GetValue<string>() ||
         response["assignment_revision"]!.GetValue<ulong>() != input["assignment"]!["revision"]!.GetValue<ulong>()))
    {
        throw new InvalidOperationException("Decision was not correlated with the evaluated assignment revision.");
    }
    var actual = new JsonObject { ["status"] = response["status"]!.DeepClone() };
    if (response["decision"] is { } decision) actual["decision"] = decision.DeepClone();
    if (response["code"] is { } code) actual["code"] = code.DeepClone();
    if (!JsonNode.DeepEquals(actual, expected))
    {
        throw new InvalidOperationException($"{test["name"]}: expected {expected}, got {actual}");
    }
    Console.WriteLine($"PASS {test["name"]}");
}
Console.WriteLine($"Passed {cases.Count} shared decision vectors through the native ABI.");
return 0;

static void Merge(JsonObject target, JsonObject patch)
{
    foreach (var (key, value) in patch)
    {
        if (value is JsonObject nested && target[key] is JsonObject existing) Merge(existing, nested);
        else target[key] = value?.DeepClone();
    }
}

internal static class Native
{
    [DllImport("psf_guard_director_ffi", EntryPoint = "psfg_director_abi_version", CallingConvention = CallingConvention.Cdecl)]
    internal static extern uint AbiVersion();

    [DllImport("psf_guard_director_ffi", EntryPoint = "psfg_director_evaluate", CallingConvention = CallingConvention.Cdecl)]
    private static extern int EvaluateJson(byte[] input, nuint inputLength, [Out] byte[]? output, nuint capacity, out nuint length);

    internal static byte[] Evaluate(byte[] input)
    {
        var status = EvaluateJson(input, (nuint)input.Length, null, 0, out var length);
        if (status != -2 || length == 0 || length > 262144)
        {
            throw new InvalidOperationException($"Native sizing failed: {status}, length {length}.");
        }
        var output = new byte[checked((int)length)];
        status = EvaluateJson(input, (nuint)input.Length, output, (nuint)output.Length, out var written);
        if (status != 0 || written != (nuint)output.Length)
        {
            throw new InvalidOperationException($"Native evaluation failed: {status}, length {written}.");
        }
        return output;
    }
}

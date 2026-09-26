using System.Runtime.InteropServices;
using System.Security.Cryptography;
using System.Text.Json;

if (args.Length != 2) throw new ArgumentException("Expected NINA SOFA DLL path and output JSON path.");
var source = Path.GetFullPath(args[0]);
var library = NativeLibrary.Load(source);
try
{
    var calendar = Marshal.GetDelegateForFunctionPointer<Calendar>(NativeLibrary.GetExport(library, "iauDtf2d"));
    var observed = Marshal.GetDelegateForFunctionPointer<Observed>(NativeLibrary.GetExport(library, "iauAtco13"));
    var samples = new List<object>();
    var times = new[] { "2016-12-31T23:59:59.999Z", "2017-01-01T00:00:00.000Z", "2026-09-26T08:00:00.125Z" };
    var sites = new[] { (35.0, -120.0, 1000.0), (-35.0, 149.0, 500.0), (89.9, 179.9, 0.0), (-89.9, -179.9, 3000.0) };
    var targets = new[] { (0.0, 0.0), (83.0, -5.0), (359.999, 89.9), (180.0, -89.9) };
    foreach (var timestamp in times)
        foreach (var (latitude, longitude, elevation) in sites)
            foreach (var (ra, dec) in targets)
            {
                var time = DateTimeOffset.Parse(timestamp, System.Globalization.CultureInfo.InvariantCulture);
                var utc = time.UtcDateTime;
                double first = 0, second = 0;
                if (calendar("UTC", utc.Year, utc.Month, utc.Day, utc.Hour, utc.Minute, utc.Second + utc.Millisecond / 1000.0, ref first, ref second) != 0)
                    throw new InvalidDataException("SOFA calendar status was not zero.");
                const double dut1 = 0.123, xp = 1e-6, yp = -2e-6;
                double az = 0, zenith = 0, hour = 0, dob = 0, rob = 0, eo = 0;
                static double Radians(double degrees) => degrees * Math.PI / 180.0;
                if (observed(Radians(ra), Radians(dec), 0, 0, 0, 0, first, second, dut1, Radians(longitude), Radians(latitude), elevation,
                    xp, yp, 0, 0, 0, 0, ref az, ref zenith, ref hour, ref dob, ref rob, ref eo) != 0)
                    throw new InvalidDataException("SOFA observed status was not zero.");
                var ms = time.ToUnixTimeMilliseconds();
                samples.Add(new
                {
                    UnixMs = ms,
                    Position = new { RaDegrees = ra, DecDegrees = dec },
                    Site = new { LatitudeDegrees = latitude, LongitudeDegrees = longitude, ElevationMeters = elevation },
                    Orientation = new { Ut1MinusUtcSeconds = dut1, PolarMotionXRadians = xp, PolarMotionYRadians = yp, ValidFromMs = ms - 3600000, ValidUntilMs = ms + 3600000 },
                    Expected = new { AzimuthDegrees = az * 180.0 / Math.PI, AltitudeDegrees = 90 - zenith * 180.0 / Math.PI, HourAngleDegrees = hour * 180.0 / Math.PI }
                });
            }
    var fixture = new { Source = Path.GetFileName(source), Sha256 = Convert.ToHexStringLower(SHA256.HashData(File.ReadAllBytes(source))), Mode = "icrs-zero-motion-zero-pressure", Samples = samples };
    File.WriteAllText(args[1], JsonSerializer.Serialize(fixture, new JsonSerializerOptions { WriteIndented = true, PropertyNamingPolicy = JsonNamingPolicy.SnakeCaseLower }) + "\n");
    Console.WriteLine($"Wrote {samples.Count} independent NINA SOFA reference positions.");
}
finally { NativeLibrary.Free(library); }

[UnmanagedFunctionPointer(CallingConvention.Cdecl, CharSet = CharSet.Ansi)]
delegate int Calendar(string scale, int year, int month, int day, int hour, int minute, double second, ref double first, ref double fraction);
[UnmanagedFunctionPointer(CallingConvention.Cdecl)]
delegate int Observed(double rc, double dc, double pr, double pd, double px, double rv, double utc1, double utc2, double dut1,
    double longitude, double latitude, double height, double xp, double yp, double pressure, double temperature, double humidity, double wavelength,
    ref double azimuth, ref double zenith, ref double hourAngle, ref double observedDec, ref double observedRa, ref double origins);

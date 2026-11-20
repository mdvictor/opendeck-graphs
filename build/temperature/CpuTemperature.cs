// Optional CPU-only host for LibreHardwareMonitor. The OpenDeck process reads one sample.
using System;
using System.Drawing;
using System.IO.MemoryMappedFiles;
using System.Linq;
using System.IO;
using System.Security.AccessControl;
using System.Security.Principal;
using System.ServiceProcess;
using System.Windows.Forms;
using LibreHardwareMonitor.Hardware;

internal static class CpuTemperature {
    const string Mapping = @"Local\OpenDeck.HardwareGraphs.CpuTemperature.v1";

    [STAThread]
    static int Main() {
        var computer = new Computer { IsCpuEnabled = true };
        try {
            using (var driver = new ServiceController("PawnIO")) {
                if (driver.Status == ServiceControllerStatus.Stopped) driver.Start();
                driver.WaitForStatus(ServiceControllerStatus.Running, TimeSpan.FromSeconds(10));
            }
            using (var map = CreateMapping())
            using (var output = map.CreateViewAccessor(0, 8, MemoryMappedFileAccess.Write))
            using (var menu = new ContextMenuStrip())
            using (var icon = new NotifyIcon { Icon = SystemIcons.Information, Text = "OpenDeck CPU sensor", ContextMenuStrip = menu })
            using (var timer = new Timer { Interval = 1000 }) {
                computer.Open();
                menu.Items.Add("Quit OpenDeck CPU sensor", null, delegate { Application.ExitThread(); });
                icon.Visible = true;
                EventHandler sample = delegate {
                    var reading = ReadTemperature(computer);
                    long packed = 0;
                    if (reading.HasValue) {
                        long hundredths = (long)Math.Round(reading.Value * 100);
                        packed = (DateTimeOffset.UtcNow.ToUnixTimeSeconds() << 32) | (uint)hundredths;
                    }
                    output.Write(0, packed);
                    icon.Text = reading.HasValue ? String.Format("OpenDeck CPU sensor: {0:F1} °C", reading) : "OpenDeck CPU sensor: no package reading";
                };
                timer.Tick += sample;
                sample(null, EventArgs.Empty);
                timer.Start();
                Application.Run();
                timer.Stop();
                output.Write(0, 0L);
                icon.Visible = false;
            }
            return 0;
        } catch (Exception error) {
            MessageBox.Show(error.Message, "OpenDeck CPU sensor", MessageBoxButtons.OK, MessageBoxIcon.Error);
            return 1;
        } finally { computer.Close(); }
    }

    static MemoryMappedFile CreateMapping() {
        var security = new MemoryMappedFileSecurity();
        security.AddAccessRule(new AccessRule<MemoryMappedFileRights>(WindowsIdentity.GetCurrent().User,
            MemoryMappedFileRights.Read, AccessControlType.Allow));
        security.AddAccessRule(new AccessRule<MemoryMappedFileRights>(new SecurityIdentifier(WellKnownSidType.BuiltinAdministratorsSid, null),
            MemoryMappedFileRights.FullControl, AccessControlType.Allow));
        return MemoryMappedFile.CreateNew(Mapping, 8, MemoryMappedFileAccess.ReadWrite,
            MemoryMappedFileOptions.None, security, HandleInheritability.None);
    }

    static float? ReadTemperature(Computer computer) {
        float? hottest = null;
        foreach (var cpu in computer.Hardware.Where(h => h.HardwareType == HardwareType.Cpu)) {
            cpu.Update();
            var sensor = cpu.Sensors.Where(s => s.SensorType == SensorType.Temperature && s.Value.HasValue)
                .OrderBy(s => Priority(s.Name)).FirstOrDefault(s => Priority(s.Name) < 3);
            if (sensor != null && sensor.Value >= 0 && sensor.Value <= 150)
                hottest = hottest.HasValue ? Math.Max(hottest.Value, sensor.Value.Value) : sensor.Value;
        }
        return hottest;
    }

    static int Priority(string name) {
        if (name.IndexOf("Tctl/Tdie", StringComparison.OrdinalIgnoreCase) >= 0) return 0;
        if (name.Equals("CPU Package", StringComparison.OrdinalIgnoreCase)) return 1;
        if (name.Equals("Core (Tdie)", StringComparison.OrdinalIgnoreCase)) return 2;
        return 3;
    }
}

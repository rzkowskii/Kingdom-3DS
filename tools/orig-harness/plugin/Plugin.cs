// KingdomTrace: a measurement harness for the original Kingdom: New Lands,
// run inside the user's own local copy of the game (see ../README.md).
//
// It does four things, all configured by the file named in KTRACE_CONFIG
// (written by `regress orig export`):
//   * fixes time (Time.captureFramerate = 60: one frame = one port tick)
//     and seeds UnityEngine.Random, so repeated runs can be compared;
//   * replaces the gamepad with a scripted input sequence, mapped to the
//     player's controls the same way the port's ControlMapper does;
//   * samples named state every N frames and logs calls to named methods;
//   * takes screenshots at given clock times ([shots]: "day hour path");
//   * can force every day's weather (force_weather = BlenderState.Weather value);
//   * quits after a fixed number of frames.
//
// Everything game-specific is looked up by name at run time. This file
// contains no code from the game.

using System;
using System.Collections.Generic;
using System.Globalization;
using System.IO;
using System.Reflection;
using System.Text;
using BepInEx;
using HarmonyLib;
using UnityEngine;

[BepInPlugin("kingdom3ds.trace", "KingdomTrace", "1.0.0")]
public class KingdomTrace : BaseUnityPlugin
{
    const float Tick = 1f / 60f;
    const float DoublePressWindow = 0.6f; // the port's controls::DOUBLE_PRESS_WINDOW

    static KingdomTrace I;
    static readonly CultureInfo Inv = CultureInfo.InvariantCulture;

    // Config.
    readonly Dictionary<string, string> cfg = new Dictionary<string, string>();
    readonly List<KeyValuePair<string, string>> samples = new List<KeyValuePair<string, string>>();
    readonly List<KeyValuePair<string, string>> setup = new List<KeyValuePair<string, string>>();
    readonly List<KeyValuePair<string, string>> hooks = new List<KeyValuePair<string, string>>();
    readonly List<int[]> script = new List<int[]>(); // per frame: dir, sprint, pay
    // Screenshots still to take: hours since day 0 00:00, and the file.
    readonly List<KeyValuePair<double, string>> shots = new List<KeyValuePair<double, string>>();
    int seed, frames, sampleEvery = 1, startTimeout;
    string startWhen;
    int waited;

    // Run state.
    StreamWriter w;
    int frame = -1;        // frames since the run started; -1 before
    int inputFrame = -2;   // the frame input was last applied on
    bool done;
    static readonly Dictionary<MethodBase, string> hookNames = new Dictionary<MethodBase, string>();
    // The citizen "@drive" spawned; sample paths starting "@driven." read it.
    static GameObject driven;

    // The port's ControlMapper, mirrored.
    float[] sincePress = { float.PositiveInfinity, float.PositiveInfinity };
    int prevDir;
    bool prevSprint, prevPay;
    MethodInfo updateActionState, updatePayState;

    void Awake()
    {
        I = this;
        string path = Environment.GetEnvironmentVariable("KTRACE_CONFIG");
        if (string.IsNullOrEmpty(path))
        {
            Logger.LogInfo("KTRACE_CONFIG not set: the harness does nothing");
            enabled = false;
            return;
        }
        Load(path);
        // Fixed time step: every frame advances game time by exactly 1/60 s,
        // however fast or slow it renders.
        Time.captureFramerate = 60;
        Application.runInBackground = true;
        QualitySettings.vSyncCount = 0;
        Application.targetFrameRate = -1;
        AudioListener.volume = 0f;

        w = new StreamWriter(cfg["out"], false, new UTF8Encoding(false));
        Line("H harness KingdomTrace 1.0.0");
        Line("H game_version " + Application.version);
        Line("H unity_version " + Application.unityVersion);
        Line("H platform " + Application.platform);
        Line("H seed " + seed);
        Line("H frames " + frames);
        Line("H start_when " + startWhen);
        Line("H sample_every " + sampleEvery);
        Line("H scenario " + Get("scenario", "?"));
        Line("H started_utc " + DateTime.UtcNow.ToString("o", Inv));
        foreach (var kv in setup) Line("H setup " + kv.Key + " = " + kv.Value);

        var harmony = new Harmony("kingdom3ds.trace");
        Patch(harmony, "Player", "IControllable.ReceiveInput", "InputPrefix", null);
        Patch(harmony, "Level", "GenerateCurrentConfig", "SeedPrefix", null);
        Patch(harmony, "Game", "StartGame", null, "StartPostfix");
        foreach (var kv in hooks)
        {
            int dot = kv.Value.LastIndexOf('.');
            Type t = AccessTools.TypeByName(kv.Value.Substring(0, dot));
            string name = kv.Value.Substring(dot + 1);
            int n = 0;
            if (t != null)
            {
                foreach (MethodInfo m in AccessTools.GetDeclaredMethods(t))
                {
                    if (m.Name != name || m.IsAbstract) continue;
                    hookNames[m] = kv.Key;
                    harmony.Patch(m, postfix: new HarmonyMethod(typeof(KingdomTrace), "HookPostfix"));
                    n++;
                }
            }
            Line("H hook " + kv.Key + " = " + kv.Value + (n == 0 ? " MISSING" : " x" + n));
        }
        w.Flush();
    }

    string Get(string k, string d) { string v; return cfg.TryGetValue(k, out v) ? v : d; }

    void Load(string path)
    {
        string section = "";
        foreach (string raw in File.ReadAllLines(path))
        {
            string line = raw.Trim();
            if (line.Length == 0 || line.StartsWith("#")) continue;
            if (line.StartsWith("[")) { section = line.Trim('[', ']'); continue; }
            if (section == "script")
            {
                // ticks dir sprint pay
                string[] p = line.Split(new[] { ' ' }, StringSplitOptions.RemoveEmptyEntries);
                int ticks = int.Parse(p[0], Inv);
                int[] f = { int.Parse(p[1], Inv), int.Parse(p[2], Inv), int.Parse(p[3], Inv) };
                for (int i = 0; i < ticks; i++) script.Add(f);
                continue;
            }
            if (section == "shots")
            {
                // day hour path
                string[] p = line.Split(new[] { ' ' }, 3, StringSplitOptions.RemoveEmptyEntries);
                double at = double.Parse(p[0], Inv) * 24.0 + double.Parse(p[1], Inv);
                shots.Add(new KeyValuePair<double, string>(at, p[2]));
                continue;
            }
            int eq = line.IndexOf('=');
            if (eq < 0) continue;
            var kv = new KeyValuePair<string, string>(line.Substring(0, eq).Trim(), line.Substring(eq + 1).Trim());
            switch (section)
            {
                case "sample": samples.Add(kv); break;
                case "setup": setup.Add(kv); break;
                case "hooks": hooks.Add(kv); break;
                default: cfg[kv.Key] = kv.Value; break;
            }
        }
        shots.Sort((a, b) => a.Key.CompareTo(b.Key));
        seed = int.Parse(Get("seed", "1"), Inv);
        frames = int.Parse(Get("frames", "600"), Inv);
        sampleEvery = Math.Max(1, int.Parse(Get("sample_every", "1"), Inv));
        // "path" (a bool) or "path == value".
        startWhen = Get("start_when", "Managers.game.state == Playing");
        startTimeout = int.Parse(Get("start_timeout", "36000"), Inv);
    }

    void Line(string s) { if (w != null) w.WriteLine(s); }

    void Patch(Harmony h, string type, string method, string prefix, string postfix)
    {
        Type t = AccessTools.TypeByName(type);
        MethodInfo m = t == null ? null : AccessTools.Method(t, method);
        if (m == null) { Line("H patch " + type + "." + method + " MISSING"); return; }
        h.Patch(m,
            prefix: prefix == null ? null : new HarmonyMethod(typeof(KingdomTrace), prefix),
            postfix: postfix == null ? null : new HarmonyMethod(typeof(KingdomTrace), postfix));
        Line("H patch " + type + "." + method);
    }

    // ---- Patches ------------------------------------------------------

    // The island is generated from UnityEngine.Random: seed it first.
    static void SeedPrefix()
    {
        if (I == null) return;
        UnityEngine.Random.InitState(I.seed);
        I.Line("H seeded_generation " + I.seed);
        I.ForceWeather();
    }

    // "force_weather = N": every Day asset offers only that weather, so
    // Day.GetRandomWeatherOption picks it (it still draws from Random, as
    // before). Runs before the director schedules its first cycle, and only
    // changes the assets in memory.
    bool weatherForced;
    void ForceWeather()
    {
        string fw = Get("force_weather", "");
        if (fw == "" || weatherForced) return;
        weatherForced = true;
        Type dayT = AccessTools.TypeByName("Day");
        FieldInfo opts = dayT == null ? null : AccessTools.Field(dayT, "weatherOptions");
        FieldInfo cache = dayT == null ? null : AccessTools.Field(dayT, "_weatherOptionsList");
        int n = 0;
        if (opts != null)
        {
            object value = Enum.ToObject(opts.FieldType, int.Parse(fw, Inv));
            foreach (UnityEngine.Object d in Resources.FindObjectsOfTypeAll(dayT))
            {
                opts.SetValue(d, value);
                if (cache != null) cache.SetValue(d, null);
                n++;
            }
        }
        Line("H weather_forced " + fw + " days " + n + (n == 0 ? " MISSING" : ""));
    }

    // The run starts here. Skip the intro as a player holding the skip
    // button would, then reseed so gameplay randomness doesn't depend on
    // how many frames loading took.
    static void StartPostfix(object __instance)
    {
        if (I == null) return;
        object state = Read(__instance, "state");
        if (state != null && state.ToString() == "Intro")
        {
            MethodInfo abort = AccessTools.Method(__instance.GetType(), "AbortIntroAndTutorial");
            if (abort != null) { abort.Invoke(__instance, null); I.Line("H intro_skipped"); }
        }
        UnityEngine.Random.InitState(I.seed * 7919 + 17);
        I.Line("H seeded_gameplay " + (I.seed * 7919 + 17));
    }

    // Scripted input instead of the gamepad.
    static bool InputPrefix(object __instance)
    {
        if (I == null || I.frame < 0 || I.done) return false;
        I.ApplyInput(__instance);
        return false;
    }

    static void HookPostfix(MethodBase __originalMethod, object __instance)
    {
        if (I == null || I.frame < 0 || I.done) return;
        string name;
        if (!hookNames.TryGetValue(__originalMethod, out name)) return;
        var o = __instance as UnityEngine.Object;
        I.Line("E " + (I.frame + 1) + " " + name + " " + (o != null ? o.GetInstanceID() : 0));
    }

    void ApplyInput(object player)
    {
        int[] f = frame < script.Count ? script[frame] : new[] { 0, 0, 0 };
        int dir = f[0];
        bool sprint = f[1] != 0, pay = f[2] != 0;
        // controls::DoubleTap::update
        for (int i = 0; i < 2; i++) sincePress[i] += Tick;
        bool pressed = dir != 0 && dir != prevDir;
        prevDir = dir;
        bool doubled = false;
        if (pressed)
        {
            int slot = dir > 0 ? 1 : 0;
            if (sincePress[slot] <= DoublePressWindow) { sincePress[slot] = float.PositiveInfinity; doubled = true; }
            else sincePress[slot] = 0f;
        }
        // controls::ControlMapper::map
        bool startSprint = doubled || (sprint && !prevSprint);
        bool stopSprint = !sprint && prevSprint;
        bool payDown = pay && !prevPay;
        prevSprint = sprint;
        prevPay = pay;

        if (updateActionState == null)
        {
            updateActionState = AccessTools.Method(player.GetType(), "UpdateActionState");
            updatePayState = AccessTools.Method(player.GetType(), "UpdatePayState");
        }
        updateActionState.Invoke(player, new object[] { dir, startSprint, stopSprint });
        updatePayState.Invoke(player, new object[] { pay, payDown });
        inputFrame = frame;
    }

    // ---- Per frame ----------------------------------------------------

    void LateUpdate()
    {
        if (done) return;
        if (frame < 0)
        {
            if (Started())
            {
                frame = 0;
                foreach (var kv in setup)
                {
                    string err = kv.Key == "@player_x" ? MovePlayer(kv.Value)
                        : kv.Key == "@drive" ? Drive(kv.Value)
                        : WritePath(kv.Key, kv.Value);
                    Line(err == null ? "H applied " + kv.Key : "H setup_failed " + kv.Key + " " + err);
                }
                Sample();
            }
            else if (++waited > startTimeout)
            {
                Line("ABORT not_started " + waited);
                w.Flush();
                w.Close();
                w = null;
                done = true;
                Application.Quit();
            }
            return;
        }
        if (inputFrame != frame) Line("E " + (frame + 1) + " Harness.InputSkipped 0");
        frame++;
        if (frame % sampleEvery == 0 || frame >= frames) Sample();
        Shoot();
        if (frame >= frames)
        {
            Line("END " + frame);
            w.Flush();
            w.Close();
            w = null;
            done = true;
            Application.Quit();
        }
        else if (frame % 600 == 0) w.Flush();
    }

    // A screenshot once the clock passes each [shots] time. Unity writes it
    // at the end of the frame, after rendering.
    void Shoot()
    {
        if (shots.Count == 0) return;
        object day = ReadPath("Managers.director.currentDay");
        object time = ReadPath("Managers.director.currentTime");
        if (day == null || time == null) return;
        double now = Convert.ToDouble(day, Inv) * 24.0 + Convert.ToDouble(time, Inv);
        while (shots.Count > 0 && now >= shots[0].Key)
        {
            string path = shots[0].Value;
            shots.RemoveAt(0);
            Type sc = AccessTools.TypeByName("UnityEngine.ScreenCapture");
            MethodInfo cap = sc == null ? null : AccessTools.Method(sc, "CaptureScreenshot", new[] { typeof(string) });
            if (cap == null) cap = AccessTools.Method(typeof(Application), "CaptureScreenshot", new[] { typeof(string) });
            if (cap == null) { Line("H shot_failed no CaptureScreenshot"); continue; }
            cap.Invoke(null, new object[] { path });
            Line("H shot " + frame + " " + Format(day) + " " + Format(time) + " " + Format(ReadPath("@blender_stack")) + " " + path);
        }
    }

    bool Started()
    {
        int eq = startWhen.IndexOf("==");
        if (eq < 0) return Format(ReadPath(startWhen)) == "1";
        return Format(ReadPath(startWhen.Substring(0, eq).Trim())) == startWhen.Substring(eq + 2).Trim();
    }

    void Sample()
    {
        var sb = new StringBuilder("S ").Append(frame);
        foreach (var kv in samples)
        {
            sb.Append(' ').Append(kv.Key).Append('=').Append(Format(ReadPath(kv.Value)));
        }
        Line(sb.ToString());
    }

    void OnApplicationQuit()
    {
        if (w != null) { Line("ABORT " + frame); w.Flush(); w.Close(); w = null; }
    }

    // "@player_x = <number or path>": put the monarch there, at rest.
    static string MovePlayer(string value)
    {
        float x;
        if (!float.TryParse(value, NumberStyles.Float, Inv, out x))
        {
            object v = ReadPath(value);
            if (v == null) return "no value at " + value;
            x = Convert.ToSingle(v, Inv);
        }
        var player = ReadPath("Managers.kingdom.player") as Component;
        if (player == null) return "no player";
        Vector3 p = player.transform.position;
        player.transform.position = new Vector3(x, p.y, p.z);
        var body = player.GetComponent<Rigidbody2D>();
        if (body != null) { body.position = new Vector2(x, body.position.y); body.velocity = Vector2.zero; }
        return null;
    }

    // "@drive = Role speed direction": spawn a citizen of that role one unit
    // right of the campfire, switch its AI off (the Role component, which
    // runs its state machine) and hold Mover.SetSpeed(speed, direction).
    // What's left is Mover and the physics body, which is what's measured.
    static string Drive(string value)
    {
        string[] p = value.Split(new[] { ' ' }, StringSplitOptions.RemoveEmptyEntries);
        if (p.Length != 3) return "want: Role speed direction";
        float speed = float.Parse(p[1], Inv);
        int dir = int.Parse(p[2], Inv);
        string field = char.ToLowerInvariant(p[0][0]) + p[0].Substring(1) + "Prefab";
        var prefab = Read(ReadPath("Managers.holder"), field) as Component;
        if (prefab == null) return "no Managers.holder." + field;
        Type pool = AccessTools.TypeByName("Pool");
        MethodInfo spawn = pool == null ? null : AccessTools.Method(pool, "SpawnGO", new[] { typeof(GameObject) });
        if (spawn == null) return "no Pool.SpawnGO(GameObject)";
        var go = spawn.Invoke(null, new object[] { prefab.gameObject }) as GameObject;
        if (go == null) return "spawn failed";
        var layer = Read(ReadPath("Managers.world"), "gameLayer") as Transform;
        if (layer != null) go.transform.parent = layer;
        object camp = ReadPath("Managers.kingdom.campfirePosition");
        var player = ReadPath("Managers.kingdom.player") as Component;
        if (camp == null || player == null) return "no campfire or player";
        Vector3 pp = player.transform.position;
        // A little above the ground: it drops onto it in the first frames.
        go.transform.position = new Vector3(Convert.ToSingle(camp, Inv) + 1f, pp.y + 0.5f, pp.z);
        var ai = go.GetComponent(p[0]) as Behaviour;
        if (ai == null) return "no " + p[0] + " component";
        ai.enabled = false;
        Component mover = go.GetComponent("Mover");
        MethodInfo set = mover == null ? null : AccessTools.Method(mover.GetType(), "SetSpeed", new[] { typeof(float), typeof(int) });
        if (set == null) return "no Mover.SetSpeed(float, int)";
        set.Invoke(mover, new object[] { speed, dir });
        driven = go;
        return null;
    }

    // ---- Reflection paths: "Type.member.member..." ----------------------

    static string Format(object v)
    {
        if (v == null) return "null";
        if (v is float) return ((float)v).ToString("R", Inv);
        if (v is double) return ((double)v).ToString("R", Inv);
        if (v is bool) return (bool)v ? "1" : "0";
        if (v is Color)
        {
            var c = (Color)v;
            return c.r.ToString("R", Inv) + "," + c.g.ToString("R", Inv) + "," + c.b.ToString("R", Inv) + "," + c.a.ToString("R", Inv);
        }
        return Convert.ToString(v, Inv).Replace(' ', '_');
    }

    static readonly Dictionary<string, MemberInfo> members = new Dictionary<string, MemberInfo>();

    static MemberInfo Member(Type t, string name)
    {
        string key = t.FullName + ":" + name;
        MemberInfo m;
        if (members.TryGetValue(key, out m)) return m;
        m = (MemberInfo)AccessTools.Field(t, name) ?? AccessTools.Property(t, name);
        members[key] = m;
        return m;
    }

    static object Read(object o, string name)
    {
        if (o == null) return null;
        MemberInfo m = Member(o.GetType(), name);
        if (m is FieldInfo) return ((FieldInfo)m).GetValue(o);
        if (m is PropertyInfo) return ((PropertyInfo)m).GetValue(o, null);
        return null;
    }

    static object ReadStatic(Type t, string name)
    {
        MemberInfo m = Member(t, name);
        if (m is FieldInfo) return ((FieldInfo)m).GetValue(null);
        if (m is PropertyInfo) return ((PropertyInfo)m).GetValue(null, null);
        return null;
    }

    static object ReadPath(string path)
    {
        try
        {
            if (path == "@blender_stack") return BlenderStack();
            string[] p = path.Split('.');
            object o;
            if (p[0] == "@driven")
            {
                // A member of the driven citizen, or one of its components by
                // type name ("@driven.Mover._moveSpeed").
                if (driven == null) return null;
                o = driven;
                for (int i = 1; i < p.Length && o != null; i++)
                {
                    object next = Read(o, p[i]);
                    if (next == null && o is GameObject) next = ((GameObject)o).GetComponent(p[i]);
                    o = next;
                }
                return o;
            }
            Type t = AccessTools.TypeByName(p[0]);
            if (t == null) return null;
            o = ReadStatic(t, p[1]);
            for (int i = 2; i < p.Length && o != null; i++) o = Read(o, p[i]);
            return o;
        }
        catch (Exception) { return null; }
    }

    // The lighting presets on the blender's stack, bottom first, as
    // "name:weight|name:weight" (orig: Blender.stack of BlenderStateFader).
    static string BlenderStack()
    {
        var stack = ReadPath("Managers.blender.stack") as System.Collections.IEnumerable;
        if (stack == null) return null;
        var sb = new StringBuilder();
        foreach (object f in stack)
        {
            var st = Read(f, "state") as UnityEngine.Object;
            object wt = Read(f, "weight");
            if (sb.Length > 0) sb.Append('|');
            sb.Append(st != null ? st.name : "null").Append(':')
              .Append(wt == null ? "?" : Convert.ToSingle(wt, Inv).ToString("0.###", Inv));
        }
        return sb.ToString();
    }

    static string WritePath(string path, string value)
    {
        try
        {
            string[] p = path.Split('.');
            Type t = AccessTools.TypeByName(p[0]);
            if (t == null) return "no type " + p[0];
            object target = null;
            Type owner = t;
            if (p.Length > 2)
            {
                target = ReadStatic(t, p[1]);
                for (int i = 2; i < p.Length - 1 && target != null; i++) target = Read(target, p[i]);
                if (target == null) return "null on the way";
                owner = target.GetType();
            }
            MemberInfo m = Member(owner, p[p.Length - 1]);
            Type mt = m is FieldInfo ? ((FieldInfo)m).FieldType : m is PropertyInfo ? ((PropertyInfo)m).PropertyType : null;
            if (mt == null) return "no member";
            object v = mt.IsEnum ? Enum.Parse(mt, value) : mt == typeof(bool) ? (object)(value == "1" || value == "true") : Convert.ChangeType(value, mt, Inv);
            if (m is FieldInfo) ((FieldInfo)m).SetValue(target, v);
            else ((PropertyInfo)m).SetValue(target, v, null);
            return null;
        }
        catch (Exception e) { return e.GetType().Name; }
    }
}

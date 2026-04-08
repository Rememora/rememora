// Mock data for browser preview (when Tauri invoke is unavailable)

const MOCK_MEMORIES = [
  {
    id: "01JAS0001",
    uri: "rememora://projects/myapp/memories/decision/chose-zustand",
    name: "Chose Zustand over Redux for state management",
    abstract: "Evaluated Redux, MobX, and Zustand. Chose Zustand for minimal boilerplate and hooks-first API.",
    overview: "After benchmarking all three options, Zustand won on bundle size (1.1KB), DX, and React 18 compatibility.",
    content: "Full evaluation notes: Redux requires too much boilerplate. MobX has magic proxies. Zustand is simple, hooks-native, and fast.",
    context_type: "memory",
    category: "decision",
    importance: 0.9,
    active_count: 14,
    tags: '["state-management","react"]',
    source_agent: "claude-code",
    source_session: null,
    created_at: new Date(Date.now() - 2 * 86400000).toISOString(),
    updated_at: new Date(Date.now() - 3600000).toISOString(),
    superseded_by: null,
  },
  {
    id: "01JAS0002",
    uri: "rememora://projects/myapp/memories/entity/stripe-api-idempotency",
    name: "Stripe API requires idempotency keys for charges",
    abstract: "All POST requests to Stripe /charges must include an Idempotency-Key header to prevent duplicate charges.",
    overview: "Stripe uses idempotency keys to safely retry requests. Keys must be unique per request and expire after 24h.",
    content: "Implementation: generate UUID v4 per charge request, store in pending_charges table, pass as Idempotency-Key header.",
    context_type: "memory",
    category: "entity",
    importance: 0.8,
    active_count: 7,
    tags: '["payments","api"]',
    source_agent: "claude-code",
    source_session: null,
    created_at: new Date(Date.now() - 5 * 86400000).toISOString(),
    updated_at: new Date(Date.now() - 2 * 86400000).toISOString(),
    superseded_by: null,
  },
  {
    id: "01JAS0003",
    uri: "rememora://projects/myapp/memories/case/ios-hermes-build-fix",
    name: "iOS build fails with Hermes + RN 0.76 — disable new arch",
    abstract: "React Native 0.76 Hermes builds crash on iOS with new architecture enabled. Fix: set newArchEnabled=false in Podfile.",
    overview: "Build failure manifests as 'HermesInternal not found' during archive. Root cause is incompatibility between Hermes JIT and new arch bridge.",
    content: "Fix in ios/Podfile: ENV['RCT_NEW_ARCH_ENABLED'] = '0'. Also run pod install --repo-update after change.",
    context_type: "memory",
    category: "case",
    importance: 0.7,
    active_count: 3,
    tags: '["ios","react-native","hermes"]',
    source_agent: "codex",
    source_session: null,
    created_at: new Date(Date.now() - 7 * 86400000).toISOString(),
    updated_at: new Date(Date.now() - 5 * 86400000).toISOString(),
    superseded_by: null,
  },
  {
    id: "01JAS0004",
    uri: "rememora://projects/myapp/memories/pattern/bdd-test-names",
    name: "Use BDD-style test names: given_when_then",
    abstract: "All tests follow Given-When-Then naming: test function describes scenario, comments mark each phase.",
    overview: "Convention adopted after test readability audit. Makes test intent clear without reading implementation.",
    content: "Example: fn search_matches_memory_by_content() with // Given: ..., // When: ..., // Then: ... comments.",
    context_type: "memory",
    category: "pattern",
    importance: 0.6,
    active_count: 11,
    tags: '["testing","conventions"]',
    source_agent: "claude-code",
    source_session: null,
    created_at: new Date(Date.now() - 10 * 86400000).toISOString(),
    updated_at: new Date(Date.now() - 86400000).toISOString(),
    superseded_by: null,
  },
  {
    id: "01JAS0005",
    uri: "rememora://projects/myapp/memories/preference/dark-mode",
    name: "User prefers dark mode in all editors and tools",
    abstract: "Dark mode is the default theme preference across all development tools and editor configurations.",
    overview: "Applies to VS Code, terminal, and all generated UIs. Light mode should never be the default.",
    content: "Set dark theme in .vscode/settings.json, terminal profile, and any generated Tailwind configs.",
    context_type: "memory",
    category: "preference",
    importance: 0.5,
    active_count: 2,
    tags: '["preferences","ui"]',
    source_agent: "claude-code",
    source_session: null,
    created_at: new Date(Date.now() - 14 * 86400000).toISOString(),
    updated_at: new Date(Date.now() - 10 * 86400000).toISOString(),
    superseded_by: null,
  },
  {
    id: "01JAS0006",
    uri: "rememora://projects/myapp/memories/decision/expo-router",
    name: "Chose expo-router over React Navigation",
    abstract: "File-based routing via expo-router chosen for consistency with Next.js patterns and simpler deep linking.",
    overview: "expo-router v3 supports typed routes, automatic deep links, and file-system convention. Team already knows Next.js patterns.",
    content: "Migration from React Navigation stack: replace Stack.Navigator with app/ directory layout, use _layout.tsx for navigation structure.",
    context_type: "memory",
    category: "decision",
    importance: 0.8,
    active_count: 9,
    tags: '["routing","react-native"]',
    source_agent: "claude-code",
    source_session: null,
    created_at: new Date(Date.now() - 3 * 86400000).toISOString(),
    updated_at: new Date(Date.now() - 86400000).toISOString(),
    superseded_by: null,
  },
  {
    id: "01JAS0007",
    uri: "rememora://projects/rememora/memories/decision/sqlite-over-postgres",
    name: "SQLite over Postgres for CLI memory store",
    abstract: "Chose SQLite for zero-config, single-file, embeddable storage. No server process needed.",
    overview: "Rememora is a CLI tool — requiring a Postgres server would add friction. SQLite with WAL mode handles concurrent reads well.",
    content: "rusqlite with bundled feature compiles SQLite into the binary. FTS5 for search, WAL for concurrency. ~3ms startup.",
    context_type: "memory",
    category: "decision",
    importance: 0.9,
    active_count: 6,
    tags: '["architecture","database"]',
    source_agent: "claude-code",
    source_session: null,
    created_at: new Date(Date.now() - 20 * 86400000).toISOString(),
    updated_at: new Date(Date.now() - 4 * 86400000).toISOString(),
    superseded_by: null,
  },
  {
    id: "01JAS0008",
    uri: "rememora://projects/rememora/memories/event/v02-shipped",
    name: "Rememora v0.2.0 shipped with encryption + TUI",
    abstract: "v0.2.0 released with SQLCipher encryption at rest and interactive TUI dashboard.",
    overview: "Major release including encryption via SQLCipher, keychain integration, TUI browser built with ratatui, and Tauri desktop app.",
    content: "Release date: 2026-04-05. Homebrew tap updated. GitHub release with pre-built binaries for macOS arm64/x64 and Linux.",
    context_type: "memory",
    category: "event",
    importance: 0.7,
    active_count: 4,
    tags: '["release","milestone"]',
    source_agent: "claude-code",
    source_session: null,
    created_at: new Date(Date.now() - 2 * 86400000).toISOString(),
    updated_at: new Date(Date.now() - 2 * 86400000).toISOString(),
    superseded_by: null,
  },
];

const MOCK_SESSIONS = [
  {
    id: "01JAS1001",
    agent: "claude-code",
    project: "myapp",
    status: "active",
    intent: "Implementing auth flow with biometric login",
    summary: "",
    working_state: "",
    started_at: new Date(Date.now() - 1800000).toISOString(),
    ended_at: null,
    parent_id: null,
  },
  {
    id: "01JAS1002",
    agent: "codex",
    project: "myapp",
    status: "transferred",
    intent: "Fix iOS build failures with Hermes",
    summary: "Identified Hermes + new arch incompatibility. Patched Podfile.",
    working_state: "Need to verify fix on CI. Pod cache may need clearing.",
    started_at: new Date(Date.now() - 7200000).toISOString(),
    ended_at: new Date(Date.now() - 3600000).toISOString(),
    parent_id: null,
  },
  {
    id: "01JAS1003",
    agent: "claude-code",
    project: "rememora",
    status: "ended",
    intent: "Add hierarchical score propagation to search",
    summary: "Implemented propagate.rs with URI tree walking, decay scoring, 8 BDD tests passing.",
    working_state: "",
    started_at: new Date(Date.now() - 86400000).toISOString(),
    ended_at: new Date(Date.now() - 82800000).toISOString(),
    parent_id: null,
  },
  {
    id: "01JAS1004",
    agent: "claude-code",
    project: "myapp",
    status: "ended",
    intent: "Set up Stripe payment integration",
    summary: "Stripe SDK integrated. Checkout flow working. Webhook handler deployed.",
    working_state: "",
    started_at: new Date(Date.now() - 172800000).toISOString(),
    ended_at: new Date(Date.now() - 169200000).toISOString(),
    parent_id: null,
  },
];

const MOCK_PROJECTS = [
  { name: "myapp", path: "/Users/dev/myapp", description: "Mobile app" },
  { name: "rememora", path: "/Users/dev/rememora", description: "Cross-agent memory CLI" },
];

const MOCK_STATS = {
  total_memories: 142,
  active_sessions: 1,
  by_project: [
    { project: "myapp", count: 87 },
    { project: "rememora", count: 43 },
    { project: "api-gateway", count: 12 },
  ],
  by_category: [
    { category: "decision", count: 38 },
    { category: "entity", count: 31 },
    { category: "pattern", count: 27 },
    { category: "case", count: 22 },
    { category: "preference", count: 14 },
    { category: "event", count: 10 },
  ],
};

const handlers = {
  get_dashboard_stats: () => MOCK_STATS,
  get_memories: ({ limit, project, category } = {}) => {
    let filtered = MOCK_MEMORIES;
    if (project) filtered = filtered.filter((m) => m.uri.includes(`/projects/${project}/`));
    if (category) filtered = filtered.filter((m) => m.category === category);
    return filtered.slice(0, limit || 10);
  },
  get_sessions: ({ limit } = {}) => MOCK_SESSIONS.slice(0, limit || 5),
  get_projects: () => MOCK_PROJECTS,
  get_memory_detail: ({ id }) => MOCK_MEMORIES.find((m) => m.id === id) || null,
  search_memories: ({ query, limit } = {}) => {
    const words = (query || "").toLowerCase().split(/\s+/).filter(Boolean);
    const matches = MOCK_MEMORIES.filter((m) => {
      const text = `${m.name} ${m.abstract} ${m.content}`.toLowerCase();
      return words.some((w) => text.includes(w));
    });
    return matches.slice(0, limit || 10).map((m, i) => ({
      context: m,
      rank: -(10 + i * 2.5),
    }));
  },
};

export async function mockInvoke(command, args = {}) {
  const handler = handlers[command];
  if (!handler) throw new Error(`Unknown command: ${command}`);
  // Simulate slight network delay for realism
  await new Promise((r) => setTimeout(r, 80));
  return handler(args);
}

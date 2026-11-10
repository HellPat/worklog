class Worklog < Formula
  desc "Native macOS daily transcript and span-labeling prototype"
  homepage "https://github.com/HellPat/worklog"
  url "https://github.com/HellPat/worklog.git", tag: "v0.1.0", revision: "96a30f0f4989a7eaf83171fae44448b7f79962ca"
  license "MIT"
  head "https://github.com/HellPat/worklog.git", branch: "main"

  depends_on "rust" => :build
  depends_on "sqldef/sqldef/sqlite3def"
  depends_on macos: :sonoma

  def install
    ENV["WORKLOG_SQLITE3DEF"] = Formula["sqldef/sqldef/sqlite3def"].opt_bin/"sqlite3def"
    ENV["CLANG_MODULE_CACHE_PATH"] = buildpath/".clang-module-cache"
    system "cargo", "install", "--locked", "--path", "spikes/gpui-recorder", "--root", prefix,
           "--bin", "team-up-gpui-spike"
    mv bin/"team-up-gpui-spike", bin/"worklog"
    pkgshare.install "spikes/gpui-recorder/schema.sql"
    prefix.install "LICENSE", "THIRD_PARTY_NOTICES.md"
  end

  def caveats
    <<~EOS
      Run `worklog` to open the native prototype.
      Use `worklog timeline --today` for headless GCF output and `worklog --help` for
      cursor/streaming commands. Speech models and the local capture runtime
      are installed separately. Data: ~/Library/Application Support/worklog/.
    EOS
  end

  test do
    assert_match "worklog 0.1.0", shell_output("#{bin}/worklog --version")
    ENV["TEAM_UP_SPIKE_DB"] = testpath/"worklog.sqlite"
    2.times { assert_match "Schema ready", shell_output("#{bin}/worklog --check-db") }
    assert_path_exists testpath/"worklog.sqlite"
  end
end

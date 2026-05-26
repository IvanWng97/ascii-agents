class AsciiAgents < Formula
  desc "Terminal pixel-art office for AI coding agents"
  homepage "https://github.com/IvanWng97/ascii-agents"
  url "https://github.com/IvanWng97/ascii-agents/archive/refs/tags/v0.3.0.tar.gz"
  sha256 "8e65856d53d190a0d4a589d40504f884af4c633139f96a007a6c3d138a6b208e"
  license "MIT"
  head "https://github.com/IvanWng97/ascii-agents.git", branch: "main"

  livecheck do
    url :stable
    strategy :github_latest
  end

  depends_on "rust" => :build

  def install
    system "cargo", "install", *std_cargo_args(path: "crates/ascii-agents")
    system "cargo", "install", *std_cargo_args(path: "crates/ascii-agents-hook")
  end

  def caveats
    <<~EOS
      To start visualizing your Claude Code sessions:
        ascii-agents install-hooks
        ascii-agents run

      Before uninstalling, remove hooks from Claude Code:
        ascii-agents uninstall-hooks
    EOS
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/ascii-agents --version")

    # Hook shim must always exit 0 — it should never block Claude Code.
    pipe_output(bin/"ascii-agents-hook", "{}", 0)
  end
end

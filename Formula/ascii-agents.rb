class AsciiAgents < Formula
  desc "Terminal pixel-art office for AI coding agents"
  homepage "https://github.com/IvanWng97/ascii-agents"
  url "https://github.com/IvanWng97/ascii-agents/archive/refs/tags/v0.2.0.tar.gz"
  sha256 "ec274de417fe306c729d4e4d67d1387753a7fe1bfe5d97ee33e28db96fcf8906"
  license "MIT"

  livecheck do
    url :stable
    regex(/^v?(\d+(?:\.\d+)+)$/i)
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
    assert_match "ascii-agents #{version}", shell_output("#{bin}/ascii-agents --version")

    # Hook shim must always exit 0 — it should never block Claude Code.
    pipe_output(bin/"ascii-agents-hook", "{}", 0)
  end
end

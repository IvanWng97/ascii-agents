class Pixtuoid < Formula
  desc "Terminal pixel-art office for AI coding agents"
  homepage "https://github.com/IvanWng97/pixtuoid"
  license "MIT"
  head "https://github.com/IvanWng97/pixtuoid.git", branch: "main"

  depends_on "rust" => :build

  def install
    system "cargo", "install", *std_cargo_args(path: "crates/pixtuoid")
    system "cargo", "install", *std_cargo_args(path: "crates/pixtuoid-hook")
  end

  def caveats
    <<~EOS
      To start visualizing your Claude Code sessions:
        pixtuoid install-hooks
        pixtuoid run

      Before uninstalling, remove hooks from Claude Code:
        pixtuoid uninstall-hooks
    EOS
  end

  test do
    assert_match "pixtuoid", shell_output("#{bin}/pixtuoid --version")
    pipe_output(bin/"pixtuoid-hook", "{}", 0)
  end
end

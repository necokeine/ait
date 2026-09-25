const path = require("node:path");
const { execFileSync } = require("node:child_process");

const EXECUTABLE_NAME = "Paseo";

exports.default = async function afterSign(context) {
  if (process.env.PASEO_DESKTOP_SMOKE !== "1") {
    return;
  }

  if (context.electronPlatformName !== "darwin") {
    return;
  }

  execFileSync(process.execPath, [path.join(__dirname, "../e2e/rust-startup.e2e.mjs")], {
    stdio: "inherit",
    env: {
      ...process.env,
      PASEO_PACKAGED_APP: path.join(context.appOutDir, `${EXECUTABLE_NAME}.app`),
    },
  });
};

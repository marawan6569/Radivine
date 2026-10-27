import { stable } from "../data/catalog";
const asset = stable.assets[0];
export const installCommands = {
  download: `curl -fLO ${asset.url}\ncurl -fLO ${stable.checksumsUrl}`,
  verify: "sha256sum --check SHA256SUMS.txt",
  install: `sudo apt install ./${asset.name}`,
  start: "radivine-control",
  service:
    "systemctl --user daemon-reload\nsystemctl --user enable --now radivine.service",
  upgrade: `sudo apt install ./${asset.name}\nsystemctl --user daemon-reload\nsystemctl --user restart radivine.service`,
  remove:
    "systemctl --user disable --now radivine.service\nsudo apt remove radivine",
  source: `git clone --branch ${stable.tag} --depth 1 https://github.com/marawan6569/Radivine.git\ncd Radivine\ncargo build --release --locked`,
};

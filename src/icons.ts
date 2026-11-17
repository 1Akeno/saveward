import plus from "@phosphor-icons/core/regular/plus.svg?raw";
import reload from "@phosphor-icons/core/regular/arrows-clockwise.svg?raw";
import sliders from "@phosphor-icons/core/regular/sliders-horizontal.svg?raw";
import save from "@phosphor-icons/core/bold/floppy-disk-bold.svg?raw";
import folder from "@phosphor-icons/core/regular/folder-open.svg?raw";
import folderPlus from "@phosphor-icons/core/regular/folder-plus.svg?raw";
import file from "@phosphor-icons/core/regular/file-plus.svg?raw";
import trash from "@phosphor-icons/core/regular/trash.svg?raw";
import undo from "@phosphor-icons/core/regular/arrow-counter-clockwise.svg?raw";
import bookmark from "@phosphor-icons/core/regular/bookmark-simple.svg?raw";
import bookmarked from "@phosphor-icons/core/fill/bookmark-simple-fill.svg?raw";
import gamepad from "@phosphor-icons/core/light/game-controller-light.svg?raw";
import close from "@phosphor-icons/core/regular/x.svg?raw";
import image from "@phosphor-icons/core/light/image-light.svg?raw";
import cloud from "@phosphor-icons/core/regular/cloud.svg?raw";
import cloudCheck from "@phosphor-icons/core/regular/cloud-check.svg?raw";
import cloudDown from "@phosphor-icons/core/regular/cloud-arrow-down.svg?raw";
import cloudUp from "@phosphor-icons/core/regular/cloud-arrow-up.svg?raw";
import cloudWarn from "@phosphor-icons/core/regular/cloud-warning.svg?raw";
import external from "@phosphor-icons/core/regular/arrow-square-out.svg?raw";

const set = {
  plus, reload, sliders, save, folder, folderPlus, file, trash, undo, bookmark, bookmarked, gamepad, close, image,
  cloud, cloudCheck, cloudDown, cloudUp, cloudWarn, external,
};
export type IconName = keyof typeof set;

export function icon(name: IconName, cls = "") {
  return set[name].replace("<svg", `<svg class="i ${cls}" aria-hidden="true"`);
}

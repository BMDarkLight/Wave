import { BiChevronDown, BiChevronUp, BiX } from "react-icons/bi";
import ContextMenu, { type ContextMenuAnchor } from "./ContextMenu";

export default function QueueContextMenu({
  index,
  queueLength,
  anchor,
  onClose,
  onMove,
  onRemove,
}: {
  index: number;
  queueLength: number;
  anchor: ContextMenuAnchor;
  onClose: () => void;
  onMove: (from: number, to: number) => void;
  onRemove: (index: number) => void;
}) {
  return (
    <ContextMenu anchor={anchor} onClose={onClose}>
      <button
        type="button"
        role="menuitem"
        disabled={index <= 0}
        onClick={() => {
          onClose();
          onMove(index, index - 1);
        }}
      >
        <BiChevronUp /> Move Up
      </button>
      <button
        type="button"
        role="menuitem"
        disabled={index >= queueLength - 1}
        onClick={() => {
          onClose();
          onMove(index, index + 1);
        }}
      >
        <BiChevronDown /> Move Down
      </button>
      <button
        type="button"
        role="menuitem"
        onClick={() => {
          onClose();
          onRemove(index);
        }}
      >
        <BiX /> Remove
      </button>
    </ContextMenu>
  );
}

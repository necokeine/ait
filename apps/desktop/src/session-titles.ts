const markdownLink = /!?\[([^\]]*)\]\([^)]*\)/g;
const instructionTag = /<\/?(?:system|developer|user|assistant|instructions?|prompt|context)[^>]*>/gi;
const htmlTag = /<[^>]+>/g;
const lineMarker = /^\s*(?:#{1,6}\s+|>+\s*|[-+*]\s+|\d+[.)]\s+|\[[ xX]\]\s+)/gm;
const leadingCommand = /^\s*\/[a-z][\w-]*\s+/i;

/** Removes presentation/instruction syntax while retaining the user's actual words. */
export function sanitizeSessionPrompt(prompt: string, maxCharacters = 2_000): string {
  const plain = prompt
    .replace(markdownLink, "$1")
    .replace(instructionTag, " ")
    .replace(htmlTag, " ")
    .replace(/^\s*```[^\n]*$/gm, " ")
    .replace(lineMarker, "")
    .replace(leadingCommand, "")
    .replace(/[*_~`]+/g, " ")
    .replace(/\s+/g, " ")
    .trim();
  return Array.from(plain).slice(0, Math.max(0, maxCharacters)).join("").trim();
}

/** Immediate sidebar title shown while the dedicated title turn runs. */
export function temporarySessionTitle(prompt: string): string {
  return sanitizeSessionPrompt(prompt, 60);
}

/** Member names override generated titles; unnamed Sessions retain the legacy fallback. */
export function sessionDisplayTitle(session: { id: string; name?: string; title?: string | null }): string {
  const name = session.name?.trim();
  if (name) return name;
  const generated = session.title?.trim();
  return generated || `Session ${session.id.slice(0, 8)}`;
}

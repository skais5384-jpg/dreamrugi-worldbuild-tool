// An undo position belongs to this draft, never to a canonical field definition.
type Item = {
  id: string;
  archived: boolean;
  restore?: boolean;
  archiveIndex?: number;
  archiveOrder?: string[];
};

// Reconcile saved undo anchors with a later explicit active-order edit. Archived
// placeholders never impose their suffix array order on the user's active order.
function draftOrder<T extends Item>(items: T[]): string[] {
  const order = items.filter((item) => !item.archived).map((item) => item.id);
  for (const item of items.filter(
    (item) => item.archived && item.archiveOrder,
  )) {
    const old = item.archiveOrder!;
    const position = old.indexOf(item.id);
    const next = old.slice(position + 1).find((id) => order.includes(id));
    const previous = old
      .slice(0, position)
      .reverse()
      .find((id) => order.includes(id));
    const index =
      next !== undefined
        ? order.indexOf(next)
        : previous !== undefined
          ? order.indexOf(previous) + 1
          : order.length;
    order.splice(index, 0, item.id);
  }
  return order;
}
function rebaseArchiveOrders<T extends Item>(items: T[]): T[] {
  const active = items.filter((item) => !item.archived).map((item) => item.id);
  const reordered = items.some(
    (item) =>
      item.archived &&
      item.archiveOrder &&
      JSON.stringify(item.archiveOrder.filter((id) => active.includes(id))) !==
        JSON.stringify(active.filter((id) => item.archiveOrder!.includes(id))),
  );
  if (!reordered) return items;
  const order = draftOrder(items);
  return items.map((item) =>
    item.archived && item.archiveOrder
      ? { ...item, archiveOrder: order }
      : item,
  );
}

export function archiveDefinition<T extends Item>(
  items: T[],
  id: string,
): (T & Item)[] {
  items = rebaseArchiveOrders(items);
  const index = items
    .filter((item) => !item.archived)
    .findIndex((item) => item.id === id);
  const order = draftOrder(items);
  return items.map((item) =>
    item.id === id
      ? {
          ...item,
          archived: true,
          restore: false,
          archiveIndex: index,
          archiveOrder: order,
        }
      : item,
  );
}

export function restoreDefinition<T extends Item>(
  items: T[],
  id: string,
  persistedArchived: boolean,
): (T & Item)[] {
  items = rebaseArchiveOrders(items);
  const target = items.find((item) => item.id === id);
  if (!target?.archived) return items;
  const active = items.filter((item) => !item.archived);
  const restored = { ...target, archived: false, restore: persistedArchived };
  const position = target.archiveOrder?.indexOf(id) ?? -1;
  const after =
    position < 0
      ? undefined
      : target.archiveOrder
          ?.slice(position + 1)
          .find((key) => active.some((item) => item.id === key));
  const before =
    position < 0
      ? undefined
      : target.archiveOrder
          ?.slice(0, position)
          .reverse()
          .find((key) => active.some((item) => item.id === key));
  const index =
    after !== undefined
      ? active.findIndex((item) => item.id === after)
      : before !== undefined
        ? active.findIndex((item) => item.id === before) + 1
        : target.archiveIndex === undefined
          ? active.length
          : Math.max(0, Math.min(target.archiveIndex, active.length));
  delete restored.archiveIndex;
  delete restored.archiveOrder;
  active.splice(index, 0, restored);
  return [
    ...active,
    ...items.filter((item) => item.archived && item.id !== id),
  ];
}

export function consumeRestorationIntents(
  body: import("../bridge/workspace").TemplateBody,
): import("../bridge/workspace").TemplateBody {
  const result = structuredClone(body);
  const consume = (fields: import("../bridge/workspace").DraftField[]) => {
    for (const field of fields) {
      delete field.restore;
      delete field.archiveIndex;
      delete field.archiveOrder;
      delete field.archiveTitle;
      if (field.configuration.kind === "group")
        consume(field.configuration.members);
      if (
        field.configuration.kind === "single_choice" ||
        field.configuration.kind === "multi_choice"
      )
        for (const option of field.configuration.options) {
          delete option.restore;
          delete option.archiveIndex;
          delete option.archiveOrder;
        }
    }
  };
  consume(result.fields);
  return result;
}

export function rebaseRestorationIntents(
  body: import("../bridge/workspace").TemplateBody,
  base: import("../bridge/types").Template,
  identities: Record<string, string>,
  sameGeneration: boolean,
) {
  if (sameGeneration) return consumeRestorationIntents(body);
  const result = structuredClone(body);
  const visit = (
    items: import("../bridge/workspace").DraftField[],
    definitions: import("../bridge/types").Field[],
  ) => {
    for (const field of items) {
      const source = definitions.find(
        (item) => item.id === (identities[field.id] ?? field.id),
      );
      if (!field.archived && source?.lifecycle === "Active")
        delete field.restore;
      if (field.configuration.kind === "group")
        visit(field.configuration.members, source?.members ?? []);
      if (
        field.configuration.kind === "single_choice" ||
        field.configuration.kind === "multi_choice"
      )
        for (const option of field.configuration.options) {
          if (
            !option.archived &&
            source?.options.some(
              (item) =>
                item.id === (identities[option.id] ?? option.id) &&
                item.lifecycle === "Active",
            )
          )
            delete option.restore;
        }
    }
  };
  visit(result.fields, base.fields);
  return result;
}

export function changeGroupMembers(
  field: import("../bridge/workspace").DraftField,
  members: import("../bridge/workspace").DraftField[],
  originalTitle: string | null | undefined,
  canonical: (id: string) => string,
): import("../bridge/workspace").DraftField {
  if (field.configuration.kind !== "group") return field;
  const intent = field.configuration.cardTitleField;
  const title =
    intent?.intent === "set"
      ? intent.value
      : intent?.intent === "unset"
        ? null
        : originalTitle;
  const member = members.find(
    (item) => title && canonical(item.id) === canonical(title),
  );
  const previous = field.configuration.members.find(
    (item) => title && canonical(item.id) === canonical(title),
  );
  const changed =
    title &&
    (intent?.intent === "set" ||
      previous?.configuration.kind === "rich_text") &&
    (!member ||
      member.configuration.kind !== "rich_text" ||
      (member.archived && intent?.intent === "set"));
  const undo =
    field.archiveTitle?.intent === "set" &&
    intent?.intent === "unset" &&
    members.some(
      (member) =>
        field.archiveTitle?.intent === "set" &&
        canonical(member.id) === canonical(field.archiveTitle.value) &&
        !member.archived &&
        member.configuration.kind === "rich_text",
    );
  return {
    ...field,
    archiveTitle: changed ? intent : undo ? undefined : field.archiveTitle,
    configuration: {
      ...field.configuration,
      members,
      cardTitleField: changed
        ? { intent: "unset" }
        : undo
          ? field.archiveTitle
          : intent,
    },
  };
}

import { t } from '@app/lib/i18n';
import { fireAndForget } from '@core/util/fireAndForget';
import { ThrownResultError } from '@core/util/result';
import type { DefinitionChangeCheck } from '@service-factory/client';
import type { DefinitionPullRequest } from '@service-factory/generated/schemas';
import { Button, TextField } from '@ui';
import { createSignal, Index, Show } from 'solid-js';
import {
  checkDraft,
  checkRequest,
  type DraftFile,
  type DraftFileProblem,
  definitionFileUrl,
  isDefinitionFile,
  MAX_CHANGE_DESCRIPTION_BYTES,
  MAX_CHANGE_TITLE_BYTES,
  type SentChange,
  sendUnder,
  sentUnchanged,
  startedBefore,
  startsAfresh,
} from '../core/definition-change';
import {
  useCheckDefinitionChangeMutation,
  useDefinitionFileReader,
  useProposeDefinitionChangeMutation,
} from '../queries/definition';
import { ExternalLink } from './definition-contents';

const FILE_PROBLEMS = {
  path: 'factoryRuns.definitionChangePathProblem',
  duplicate: 'factoryRuns.definitionChangeDuplicateProblem',
  json: 'factoryRuns.definitionChangeJsonProblem',
} as const satisfies Record<DraftFileProblem, string>;

/**
 * What a refused change says, and whether the next send needs a new change
 * id: the pull request it would answer was closed, or its branch is there
 * with none.
 */
function refusal(error: unknown): { text: string; newChange: boolean } {
  const first =
    error instanceof ThrownResultError ? error.errors.at(0) : undefined;
  const message = first?.message ?? '';
  switch (first?.code) {
    case 'INVALID_DEFINITION_CHANGE':
      return {
        text: t('factoryRuns.definitionChangeInvalid', { message }),
        newChange: false,
      };
    case 'NO_PUSH_ACCESS':
      return {
        text: t('factoryRuns.definitionChangeNoPush'),
        newChange: false,
      };
    case 'DEFINITION_UNREACHABLE':
      return {
        text: t('factoryRuns.definitionChangeUnreachable', { message }),
        newChange: false,
      };
    case 'DEFINITION_INVALID':
      return {
        text: t('factoryRuns.definitionChangeHeadInvalid', { message }),
        newChange: false,
      };
    case 'NO_DEFINITION':
      return {
        text: t('factoryRuns.definitionChangeNoDefinition'),
        newChange: false,
      };
    case 'DEFINITION_CHANGE_NOT_OPENED':
      return {
        text: t('factoryRuns.definitionChangeNotOpened', { message }),
        newChange: true,
      };
    case 'DEFINITION_CHANGE_CLOSED':
      return { text: t('factoryRuns.definitionChangeClosed'), newChange: true };
    case 'FORBIDDEN':
      return {
        text: t('factoryRuns.definitionChangeForbidden'),
        newChange: false,
      };
    default:
      // The answer may have been lost: sent again under the same id, the
      // change answers the pull request it opened.
      return {
        text: t('factoryRuns.definitionChangeFailed'),
        newChange: false,
      };
  }
}

const EMPTY_FILE: DraftFile = { path: '', text: '' };

/** The service's words for a failed read, if it gave any. */
function readFailure(error: unknown): string {
  const first =
    error instanceof ThrownResultError ? error.errors.at(0) : undefined;
  return first?.message ?? '';
}

/**
 * A change to the team's Factory Definition, for its admins: the whole new
 * text of some of its files, sent as a pull request on its repository that
 * a person merges. Picking a file's path fills in its text as it is at the
 * Definition's head. The factory checks the Definition reads with the
 * change, on its own or before it is sent, and says why when it doesn't.
 */
export function DefinitionChangeEditor(props: {
  repoUrl: string;
  /** The commit of its default branch the change goes on top of. */
  commit?: string | null;
}) {
  const mutation = useProposeDefinitionChangeMutation();
  const checking = useCheckDefinitionChangeMutation();
  const readFile = useDefinitionFileReader();
  const busy = () => mutation.isPending || checking.isPending;
  const [editing, setEditing] = createSignal(false);
  const [title, setTitle] = createSignal('');
  const [description, setDescription] = createSignal('');
  const [files, setFiles] = createSignal<DraftFile[]>([EMPTY_FILE]);
  // What was last sent, and under which id: sent again unchanged, as after
  // a lost answer, it keeps the id and answers the pull request it opened;
  // any edit since goes under a new id, since the factory answers a resent
  // id from its pull request without writing the edit.
  const [sent, setSent] = createSignal<SentChange>();
  const [checked, setChecked] = createSignal(false);
  const [refused, setRefused] = createSignal<string>();
  const [opened, setOpened] = createSignal<DefinitionPullRequest>();
  const [passed, setPassed] = createSignal<DefinitionChangeCheck>();
  // The reads in flight, each by its own id, with the row it fills.
  const [reads, setReads] = createSignal<ReadonlyMap<number, number>>(
    new Map()
  );
  let nextRead = 0;
  const reading = (index: number) => [...reads().values()].includes(index);
  // The files that started from an older commit than the head the factory
  // last checked against, once said: sending again goes ahead.
  const [moved, setMoved] = createSignal<{ commit: string; paths: string[] }>();

  const draft = () => ({
    title: title(),
    description: description(),
    files: files(),
  });
  const shown = () => (checked() ? checkDraft(draft()) : undefined);

  const reset = () => {
    setTitle('');
    setDescription('');
    setFiles([EMPTY_FILE]);
    setSent(undefined);
    setChecked(false);
    setRefused(undefined);
    setPassed(undefined);
    setMoved(undefined);
  };

  const updateFile = (index: number, change: Partial<DraftFile>) => {
    setPassed(undefined);
    setFiles((current) =>
      current.map((file, at) => (at === index ? { ...file, ...change } : file))
    );
  };

  const startRead = (index: number) => {
    const id = nextRead++;
    setReads((current) => new Map(current).set(id, index));
    return id;
  };
  const endRead = (id: number) =>
    setReads((current) => {
      const next = new Map(current);
      next.delete(id);
      return next;
    });

  // A path picked for a file nothing was written in yet starts it from the
  // file as it is at the Definition's head; an answer for a path changed
  // since, or a file written in meanwhile, is dropped.
  const pickPath = async (index: number, value: string) => {
    updateFile(index, { path: value, readProblem: undefined });
    const path = value.trim();
    const file = files()[index];
    if (!file || !isDefinitionFile(path) || !startsAfresh(file)) return;
    const current = () => files()[index];
    const stillPicked = () => {
      const now = current();
      return !!now && now.path.trim() === path && startsAfresh(now);
    };
    const id = startRead(index);
    try {
      const read = await readFile(path);
      if (!stillPicked()) return;
      const text = read.text ?? null;
      updateFile(index, {
        text: text ?? '',
        start: { path, commit: read.commit, text },
      });
    } catch (error) {
      if (!stillPicked()) return;
      updateFile(index, { readProblem: readFailure(error) });
    } finally {
      endRead(id);
    }
  };

  // Asks the factory whether the Definition reads with the draft, writing
  // nothing; says why when it doesn't.
  const checkWithFactory = async (): Promise<
    DefinitionChangeCheck | undefined
  > => {
    try {
      const passing = await checking.mutateAsync(checkRequest(draft()));
      setPassed(passing);
      return passing;
    } catch (error) {
      const answer = refusal(error);
      setRefused(answer.text);
      if (answer.newChange) setSent(undefined);
      return undefined;
    }
  };

  // Whether files read at an older commit than `head` are newly found:
  // said once, after which sending goes ahead.
  const warnMoved = (head: string): boolean => {
    const paths = startedBefore(files(), head);
    const said = moved();
    if (paths.length === 0) {
      setMoved(undefined);
      return false;
    }
    const known =
      said?.commit === head && said.paths.join('\n') === paths.join('\n');
    setMoved({ commit: head, paths });
    return !known;
  };

  const check = async () => {
    setChecked(true);
    setRefused(undefined);
    setPassed(undefined);
    if (!checkDraft(draft()).ready || busy()) return;
    const passing = await checkWithFactory();
    if (passing) warnMoved(passing.commit);
  };

  const send = async () => {
    setChecked(true);
    setRefused(undefined);
    if (!checkDraft(draft()).ready || busy()) return;
    // A change sent again unchanged, as after a lost answer, goes as it is:
    // the factory answers its pull request, though it may since have
    // merged, which a check would refuse as changing nothing. Any other
    // draft is checked first, so one that doesn't read never takes a
    // change id, and one written from an older head is said to be.
    if (!sentUnchanged(sent(), draft())) {
      const passing = await checkWithFactory();
      if (!passing || warnMoved(passing.commit)) return;
    }
    const change = sendUnder(sent(), draft(), () => crypto.randomUUID());
    setSent(change.sent);
    try {
      const pullRequest = await mutation.mutateAsync(change.request);
      setOpened(pullRequest);
      reset();
      setEditing(false);
    } catch (error) {
      const answer = refusal(error);
      setRefused(answer.text);
      if (answer.newChange) setSent(undefined);
    }
  };

  return (
    <section class="flex flex-col gap-2 border-t border-edge-muted pt-3">
      <Show when={opened()}>
        {(pullRequest) => (
          <p class="text-xs" role="status">
            {t('factoryRuns.definitionChangeOpened')}{' '}
            <ExternalLink href={pullRequest().url}>
              {t('factoryRuns.definitionPullRequest', {
                number: pullRequest().number,
                title: pullRequest().title,
              })}
            </ExternalLink>
          </p>
        )}
      </Show>
      <Show
        when={editing()}
        fallback={
          <div class="flex justify-end">
            <Button
              variant="secondary"
              size="sm"
              onClick={() => {
                setOpened(undefined);
                setEditing(true);
              }}
            >
              {t('factoryRuns.definitionChangeStart')}
            </Button>
          </div>
        }
      >
        <fieldset
          class="flex flex-col gap-3"
          disabled={busy()}
          aria-label={t('factoryRuns.definitionChangeTitle')}
        >
          <legend class="text-sm font-medium">
            {t('factoryRuns.definitionChangeTitle')}
          </legend>
          <p class="text-xs text-ink-muted">
            {t('factoryRuns.definitionChangeHowTo')}
          </p>
          <TextField
            value={title()}
            onChange={setTitle}
            required
            validationState={shown()?.title ? 'invalid' : 'valid'}
          >
            <TextField.Label class="text-sm">
              {t('factoryRuns.definitionChangeSummary')}
            </TextField.Label>
            <TextField.Input />
            <TextField.ErrorMessage>
              {shown()?.title === 'tooLong'
                ? t('factoryRuns.definitionChangeSummaryTooLong', {
                    max: MAX_CHANGE_TITLE_BYTES,
                  })
                : t('factoryRuns.definitionChangeSummaryBlank')}
            </TextField.ErrorMessage>
          </TextField>
          <TextField
            value={description()}
            onChange={setDescription}
            validationState={shown()?.description ? 'invalid' : 'valid'}
          >
            <TextField.Label class="text-sm">
              {t('factoryRuns.definitionChangeWhy')}
            </TextField.Label>
            <TextField.TextArea autoResize rows={2} />
            <TextField.ErrorMessage>
              {t('factoryRuns.definitionChangeWhyTooLong', {
                max: MAX_CHANGE_DESCRIPTION_BYTES,
              })}
            </TextField.ErrorMessage>
          </TextField>
          <Index each={files()}>
            {(file, index) => {
              const problem = () => shown()?.files[index];
              const path = () => file().path.trim();
              return (
                <div class="flex flex-col gap-1.5 rounded-md border border-edge-muted p-2">
                  <TextField
                    value={file().path}
                    onChange={(value) =>
                      fireAndForget(pickPath(index, value), 'factory-runs')
                    }
                    validationState={
                      problem() === 'path' || problem() === 'duplicate'
                        ? 'invalid'
                        : 'valid'
                    }
                  >
                    <TextField.Label class="text-sm">
                      {t('factoryRuns.definitionChangeFile', {
                        number: index + 1,
                      })}
                    </TextField.Label>
                    <TextField.Input
                      class="font-mono"
                      placeholder={t(
                        'factoryRuns.definitionChangeFilePlaceholder'
                      )}
                    />
                    <TextField.ErrorMessage>
                      {t(FILE_PROBLEMS[problem() ?? 'path'])}
                    </TextField.ErrorMessage>
                  </TextField>
                  <Show when={reading(index)}>
                    <p class="text-xs text-ink-muted" role="status">
                      {t('factoryRuns.definitionChangeReading')}
                    </p>
                  </Show>
                  <Show
                    when={
                      file().start?.path === path() &&
                      file().start?.text === null
                    }
                  >
                    <p class="text-xs text-ink-muted">
                      {t('factoryRuns.definitionChangeNewFile')}
                    </p>
                  </Show>
                  <Show when={file().readProblem !== undefined}>
                    <p class="text-xs text-failure" role="alert">
                      {t('factoryRuns.definitionChangeReadFailed', {
                        message: file().readProblem ?? '',
                      })}
                    </p>
                  </Show>
                  <Show when={problem() !== 'path' && path()}>
                    <p class="text-xs text-ink-muted">
                      <ExternalLink
                        href={definitionFileUrl(
                          props.repoUrl,
                          path(),
                          props.commit
                        )}
                      >
                        {t('factoryRuns.definitionChangeCurrentFile')}
                      </ExternalLink>
                    </p>
                  </Show>
                  <TextField
                    value={file().text}
                    onChange={(value) => updateFile(index, { text: value })}
                    validationState={problem() === 'json' ? 'invalid' : 'valid'}
                  >
                    <TextField.Label class="text-sm">
                      {t('factoryRuns.definitionChangeFileText', {
                        number: index + 1,
                      })}
                    </TextField.Label>
                    <TextField.TextArea
                      class="max-h-96 overflow-y-auto font-mono text-xs"
                      autoResize
                      rows={6}
                      spellcheck={false}
                    />
                    <TextField.ErrorMessage>
                      {t(FILE_PROBLEMS.json)}
                    </TextField.ErrorMessage>
                  </TextField>
                  <Show when={files().length > 1}>
                    <div class="flex justify-end">
                      <Button
                        variant="ghost"
                        size="xs"
                        onClick={() =>
                          setFiles((current) =>
                            current.filter((_, at) => at !== index)
                          )
                        }
                      >
                        {t('factoryRuns.definitionChangeRemoveFile', {
                          number: index + 1,
                        })}
                      </Button>
                    </div>
                  </Show>
                </div>
              );
            }}
          </Index>
          <Show when={passed()}>
            {(check) => (
              <p class="text-xs" role="status">
                {t('factoryRuns.definitionChangeChecked', {
                  commit: check().commit.slice(0, 7),
                })}
              </p>
            )}
          </Show>
          <Show when={moved()}>
            {(behind) => (
              <p class="text-xs text-failure" role="alert">
                {t('factoryRuns.definitionChangeMoved', {
                  commit: behind().commit.slice(0, 7),
                  paths: behind().paths.join(', '),
                })}
              </p>
            )}
          </Show>
          <Show when={refused()}>
            {(text) => (
              <p class="text-xs whitespace-pre-wrap text-failure" role="alert">
                {text()}
              </p>
            )}
          </Show>
          <div class="flex flex-wrap justify-end gap-2">
            <Button
              variant="ghost"
              size="sm"
              onClick={() => setFiles((current) => [...current, EMPTY_FILE])}
            >
              {t('factoryRuns.definitionChangeAddFile')}
            </Button>
            <Button
              variant="secondary"
              size="sm"
              onClick={() => {
                reset();
                setEditing(false);
              }}
            >
              {t('factoryRuns.definitionChangeDiscard')}
            </Button>
            <Button
              variant="secondary"
              size="sm"
              loading={checking.isPending && !mutation.isPending}
              onClick={() => fireAndForget(check(), 'factory-runs')}
            >
              {t('factoryRuns.definitionChangeCheck')}
            </Button>
            <Button
              variant="primary"
              size="sm"
              loading={mutation.isPending}
              onClick={() => fireAndForget(send(), 'factory-runs')}
            >
              {t('factoryRuns.definitionChangeSend')}
            </Button>
          </div>
        </fieldset>
      </Show>
    </section>
  );
}

{-# LANGUAGE FlexibleContexts #-}
{-# LANGUAGE OverloadedLabels #-}
{-# LANGUAGE OverloadedRecordDot #-}
{-# LANGUAGE OverloadedStrings #-}

-- | This project's own after-tool nudge layer, per @docs/nudges.md@. Distinct
-- from the vendored 'Project.Watchdog' (nouls only, parent-installed per
-- child label): this module is the project-specific handler the doc asks
-- for, with a choice gate (L0), the \"everyone\" hygiene battery (L1), a
-- single selection (L6), and one strict-settled escalation (L7,
-- @destructive_command@, reusing 'Watchdog''s own wording).
--
-- Scope (see the top-level report for the full accounting): the doc's label
-- sets beyond \"everyone\" (@core-*@, @server-*@, @web-*@, @lead@, @root@)
-- name rubrics for subsystems -- transport, store, loop, compaction, the
-- ws/live/forms/demo servers, and the tree/timeline/window/inbox/palette
-- screens -- that have no built module in this checkout yet, and no actor is
-- spawned under those labels by anything in this project today; there is
-- nothing for those batteries to gate on. This module installs \"everyone\"
-- for every actor, which is exactly what the doc says every label gets
-- regardless of its other sets. 'candidatesFor' is the seam a later
-- label-specific set extends without touching the handler or the ledger.
--
-- The ledger uses this project's own 'Journal' effect
-- ('Project.FieldNotes' is the existing user of it in the vendored
-- workspace) rather than a hand-rolled file write: the root ownership map
-- names a durable log as a cross-crate mechanism with one implementation,
-- and Journal already is it.
module Project.Nudges
  ( watch
  , NudgeAnswers (..)
  , L0 (..)
  , Candidate (..)
  , Disposition (..)
  , candidatesFor
  , antiNag
  , selectAdvice
  , handleAnswers
  , ledgerJson
  ) where

import Control.Monad (void)
import Control.Monad.Freer (Eff, Member)
import Data.Text (Text)
import qualified Data.Text as T
import Tidepool.Aeson.Value (Value (..), object, (.=))
import Tidepool.Agent.Contract
import Tidepool.Actors.Exomonad (parentAgent, sendMessage)
import Tidepool.Effects.Core
  ( ActorContext, ActorContextInfo (..), ConversationTurn (..), Jev, Journal, Notifications, Reflect
  , TurnItem (..), actorContext, reflect
  )
import Tidepool.Journal (record)
import qualified Jev.Operators as J
import Jev.Operators (Packet ((:=), (:&)))
import qualified Project.Watchdog as Watchdog

-- ---------------------------------------------------------------------------
-- L0: the gate choices. No advice of their own; every L1 item's applicability
-- is a plain predicate over these three answers.
-- ---------------------------------------------------------------------------

data L0 = L0 { l0Kind :: Text, l0Phase :: Text, l0Artifact :: Text }
  deriving (Show, Eq)

-- ---------------------------------------------------------------------------
-- L1 "everyone": the doc's hygiene nouls, question text authored verbatim
-- from docs/nudges.md. 'NudgeAnswers' is what a caller reads back from the
-- Jev response, and is also exactly what the replay test constructs
-- directly, so the selection and anti-nag logic never has to touch a live
-- Jev response to be tested.
-- ---------------------------------------------------------------------------

data NudgeAnswers = NudgeAnswers
  { l0 :: L0
  , stagesEverything :: Double
  , doneWithStubs :: Double
  , unwrapInLib :: Double
  , stringlyTyped :: Double
  , blockingInAsync :: Double
  , unboundedChannel :: Double
  , orphanTask :: Double
  , secretInOutput :: Double
  , hardcodedLimit :: Double
  , helperShipped :: Double
  , noncanonicalJson :: Double
  , reinventsLibrary :: Double
  , repeatingItself :: Double
  , ignoringAFailure :: Double
  , asksAnsweredQuestion :: Double
  , askShapeBad :: Double
  , investedScope :: Double
  , interviewMissing :: Double
  , destructiveCleared :: Bool
    -- ^ L7: whether 'J.settle' 'J.strict' cleared the destructive choice.
    -- 'False' when settling did not clear -- the doc's "otherwise the
    -- escalation noul degrades to advice", which 'candidatesFor' already
    -- carries as the plain 'destructive_command' wording reused from
    -- 'Watchdog' at ordinary noul strength (this project has no separate
    -- lenient re-ask for the same question, so the degrade is: no escalation,
    -- and the underlying call is still visible in @result@/@recent_calls@
    -- for the next nudge and for 'Watchdog''s own core heuristics, which stay
    -- installed independently).
  } deriving (Show, Eq)

-- | One candidate advice, named by its heuristic key (matches
-- @docs/nudges.md@'s item names) so the ledger and anti-nag can address it.
data Candidate = Candidate { candidateKey :: Text, candidateAdvice :: Text }
  deriving (Show, Eq)

data Disposition
  = DAbstained Text
  | DAnnotated Text [Text]
    -- ^ the one shown line, plus escalated candidate keys (never more than
    -- one shown line; escalation is additional, never suppressed by
    -- anti-nag, per the doc).
  deriving (Show, Eq)

floor60 :: Double -> Bool
floor60 mass = mass >= 0.6

floorAt :: Double -> Double -> Bool
floorAt f mass = mass >= f

adviceOf :: Watchdog.Heuristic -> Text
adviceOf h = case Watchdog.heuristicOutcome h of
  Watchdog.Advise t -> t
  Watchdog.Escalate t -> t

-- | The candidates a battery's answers trip, gated by L0 where the doc names
-- a @(kind=…)@ / @(artifact=…)@ condition, in the doc's own order (so
-- 'selectAdvice' picking the first survivor matches the doc's severity tie
-- order for a set that carries no scores).
candidatesFor :: NudgeAnswers -> [Candidate]
candidatesFor a = concat
  [ trip "stages_everything" stagesEverything floor60
      "siblings share index; git commit -F msg -- <paths>" always
  , trip "done_with_stubs" doneWithStubs floor60
      "finish or state exactly what is left" (kindIs "reply")
  , trip "unwrap_in_lib" unwrapInLib floor60
      "typed error; propagate" (artifactIs "rust_lib")
  , trip "stringly_typed" stringlyTyped floor60
      "use the newtype/enum that already exists" always
  , trip "blocking_in_async" blockingInAsync floor60
      "no blocking in async" always
  , trip "unbounded_channel" unboundedChannel floor60
      "bounded + overflow policy" always
  , trip "orphan_task" orphanTask floor60
      "token hierarchy run\8594conversation\8594request/job" always
  , trip "secret_in_output" secretInOutput floor60
      "redact at transport boundary" always
  , trip "hardcoded_limit" hardcodedLimit floor60
      "config field with a default" always
  , trip "helper_shipped" helperShipped (floorAt 0.75)
      "primitives not helpers; delete" always
  , trip "noncanonical_json" noncanonicalJson floor60
      "canonical serializer for every hashed/cached byte" always
  , trip "reinvents_library" reinventsLibrary (floorAt 0.7)
      "use the library from the stack, or write the reason down" always
  , trip "repeating_itself" repeatingItself floor60
      (adviceOf Watchdog.repeatingItself) always
  , trip "ignoring_a_failure" ignoringAFailure floor60
      (adviceOf Watchdog.ignoringAFailure) always
  , trip "asks_answered_question" asksAnsweredQuestion floor60
      "cite the PRD/tree.md section; don't ask" (kindIs "ask")
  , trip "ask_shape" askShapeBad floor60
      "tree.md ask shape: [label], default:, blocks:" (kindIs "ask")
  , trip "invented_scope" investedScope floor60
      "stop; ask; the PRD assignment stands until amended" always
  , trip "interview_missing" interviewMissing floor60
      "the interview is a deliverable" (kindIs "reply")
  ]
  where
    always = const True
    kindIs k l0' = l0Kind l0' == k
    artifactIs k l0' = l0Artifact l0' == k
    trip key getMass tripsAt advice gate =
      [ Candidate key advice | gate (l0 a), tripsAt (getMass a) ]

-- | Drop a candidate already shown in @recent_advice@ (anti-nag), unless the
-- doc's escape hatch applies -- a score fell \8805 1 level since last time.
-- This project ships no L4/L5 scores yet, so the escape hatch never fires:
-- the drop is unconditional, which is the safe direction (never nag twice
-- with the same words; never silently suppress something whose evidence
-- just got stronger, since nothing here claims to track that yet).
antiNag :: [Text] -> [Candidate] -> [Candidate]
antiNag recentAdvice = filter (\c -> candidateAdvice c `notElem` recentAdvice)

-- | Select at most one advice line to show: the doc's tie order is score
-- deficit \8805 2 \8250 noul \8250 score deficit 1; this battery carries no
-- scores, so among the survivors the first in 'candidatesFor'\'s own
-- (doc) order wins.
selectAdvice :: [Candidate] -> Maybe Candidate
selectAdvice [] = Nothing
selectAdvice (c : _) = Just c

-- | The whole deterministic step, replay-testable without Jev: candidates,
-- anti-nag, select one, decide the escalation line. Never annotates on
-- @kind = read@.
handleAnswers :: NudgeAnswers -> [Text] -> Disposition
handleAnswers a recentAdvice
  | l0Kind (l0 a) == "read" = DAbstained "kind = read; nothing to nudge"
  | otherwise =
      let survivors = antiNag recentAdvice (candidatesFor a)
          escalated = [ "destructive_command" | destructiveCleared a ]
      in case (selectAdvice survivors, escalated) of
           (Nothing, []) -> DAbstained "no candidate stands after anti-nag"
           (shown, esc) ->
             let line = maybe "" (("\9873 " <>) . candidateAdvice) shown
                 tailLine = if null esc then "" else "\nescalated to your parent: " <> T.intercalate ", " esc
             in DAnnotated (line <> tailLine) esc

-- ---------------------------------------------------------------------------
-- The Jev ask
-- ---------------------------------------------------------------------------

-- | How many of the actor's own completed turns feed @recent_calls@ /
-- @recent_advice@. Per the doc: leaf 2, lead/root 4. This project has no
-- label-routing infrastructure yet (see the module header), so every actor
-- reads at the leaf depth; a later label seam can widen it per path.
historyDepth :: Int
historyDepth = 2

evidenceChars :: Int
evidenceChars = 8000

boundedEvidence :: Text -> Text
boundedEvidence text
  | T.length text <= evidenceChars = text
  | otherwise =
      T.take evidenceChars text
        <> "\n[truncated; " <> T.pack (show (T.length text - evidenceChars)) <> " more characters omitted]"

-- | @recent_calls@ (bounded, per the same reasoning as
-- 'Project.Watchdog.recentToolActivity') and @recent_advice@ -- the
-- \9873-prefixed lines a prior nudge already showed this actor, per
-- 'Disposition'\'s own line format (mirrors the doc's stated ledger prefix).
recentContext :: Member Reflect effects => Eff effects (Value, [Text])
recentContext = do
  turns <- reflect historyDepth
  case turns of
    Left _ -> pure (object ["availability" .= ("unavailable" :: Text)], [])
    Right ts ->
      let results = [ out | t <- ts, TurnToolResult _ out <- turnItems t ]
          advised = [ T.drop 2 line
                    | out <- results, line <- T.lines out, "\9873 " `T.isPrefixOf` line ]
          calls = object
            [ "availability" .= ("completed turns only; current turn excluded" :: Text)
            , "calls" .=
                [ object ["tool" .= name, "arguments" .= arguments, "result" .= boundedEvidence out]
                | t <- ts
                , TurnToolCall callId name arguments <- turnItems t
                , TurnToolResult callId' out <- turnItems t
                , callId == callId'
                ]
            ]
      in pure (calls, advised)

-- | The one call: L0 gate, L1 everyone, and an L7 escalation choice settled
-- 'J.strict'. Never fails the hook: a Jev failure abstains exactly like a
-- missing heuristic, and 'Watchdog.trivialCall' (reused as-is) skips the ask
-- entirely for a call this cheap to judge by inspection.
watch
  :: (Member Jev effects, Member ActorContext effects, Member Notifications effects, Member Reflect effects, Member Journal effects)
  => ToolCall -> ToolResult -> Eff effects Annotation
watch call result = do
  context <- actorContext
  case Watchdog.trivialCall call result of
    Just reason -> pure (Abstained reason)
    Nothing -> do
      (recentCalls, recentAdvice) <- recentContext
      let packet =
            #kind := J.choice "Which describes this call?"
              ( J.alt #check "Runs a build, test, typecheck, or lint." ("check" :: Text)
                J..| J.alt #commit "A git commit." "commit"
                J..| J.alt #fork "Creates a child actor." "fork"
                J..| J.alt #reply "Settles this actor's own assignment." "reply"
                J..| J.alt #edit_contract "Edits the contract/scaffold this actor doesn't own alone." "edit_contract"
                J..| J.alt #edit_own "Edits a file this actor owns." "edit_own"
                J..| J.alt #read "Reads a file, log, or command output; no write." "read"
                J..| J.alt #ask "A question addressed to the operator." "ask"
                J..| J.alt #other "None of the above." "other" )
              :& #phase := J.choice
                   "Which phase of the assignment does recent_calls plus this call show?"
                   ( J.alt #orient "Reading scaffold/PRD, no edits yet." ("orient" :: Text)
                     J..| J.alt #implement "Edits, no check since." "implement"
                     J..| J.alt #verify "Checks/tests after edits." "verify"
                     J..| J.alt #deliver "Commit/reply." "deliver"
                     J..| J.alt #blocked "Repeated failure or waiting." "blocked"
                     J..| J.alt #unclear "None of the above fits." "unclear" )
              :& #artifact := J.choice "What is being written, if anything?"
                   ( J.alt #rust_lib "Rust library code." ("rust_lib" :: Text)
                     J..| J.alt #rust_test "Rust test code." "rust_test"
                     J..| J.alt #sql "SQL (schema or query)." "sql"
                     J..| J.alt #ts_component "A TypeScript UI component." "ts_component"
                     J..| J.alt #ts_types "TypeScript types." "ts_types"
                     J..| J.alt #doc "Documentation." "doc"
                     J..| J.alt #commit_msg "A commit message." "commit_msg"
                     J..| J.alt #cmd "A shell command with no source edit." "cmd"
                     J..| J.alt #nothing "Nothing is being written." "nothing" )
              :& #stages_everything := J.noul
                   "git add/commit with -a, -A, . or --all instead of named paths?"
              :& #done_with_stubs := J.noul
                   "Does result or recent_calls show a todo!() in the delivered module, or a failing check after the last edit?"
              :& #unwrap_in_lib := J.noul
                   "Does non-test rust code call unwrap/expect/panic!?"
              :& #stringly_typed := J.noul
                   "Does this call pass an id/model/effort/kind as String/&str where a newtype or enum already exists for it?"
              :& #blocking_in_async := J.noul
                   "A blocking API (std fs/net, rusqlite, thread sleep, block_on) inside async code without spawn_blocking or a writer task?"
              :& #unbounded_channel := J.noul
                   "An unbounded channel or an uncapped queue?"
              :& #orphan_task := J.noul
                   "tokio::spawn without an owning scope or cancellation token?"
              :& #secret_in_output := J.noul
                   "Does the logged, stored, or emitted text carry an API key or auth header?"
              :& #hardcoded_limit := J.noul
                   "Hardcodes a cap, timeout, or threshold instead of a config field with a default?"
              :& #helper_shipped := J.noul
                   "Adds a public function whose body only chains two or more agent primitives (wait_agent, cancel, spawn_agent, checkpoint, set-effort, store queries) under a name that is not on the PRD primitives line?"
              :& #noncanonical_json := J.noul
                   "Hashes or sends JSON without canonical key order?"
              :& #reinvents_library := J.noul
                   "Hand-writes something nontrivial that a well-tested crate or package already provides, with no stated reason in the code or commit?"
              :& #repeating_itself := J.noul (Watchdog.heuristicQuestion Watchdog.repeatingItself)
              :& #ignoring_a_failure := J.noul (Watchdog.heuristicQuestion Watchdog.ignoringAFailure)
              :& #asks_answered_question := J.noul
                   "Is the question's answer already stated in PRD.md, tree.md, or the scaffold docs?"
              :& #ask_shape := J.noul
                   "Does the question lack [label], default:, or blocks:?"
              :& #invented_scope := J.noul
                   "Is this call building identity, auth, sessions, rate limits, or another subsystem the PRD assigns elsewhere or does not name?"
              :& #interview_missing := J.noul
                   "Is this a final reply without an interview section, or does the wave end with no docs/interviews.md entry?"
              :& #destructive := J.choice
                   "Is this call a command that deletes, force-pushes, resets, or otherwise discards work irreversibly?"
                   ( J.alt #yes "Yes, it is destructive in that sense." (True :: Bool)
                     J..| J.alt #no "No." False )
      answer <-
        J.ask
          (J.rawState (object
            [ "tool" .= toolCallName call
            , "arguments" .= toolCallArguments call
            , "result" .= boundedEvidence (toolResultOutput result)
            , "recent_calls" .= recentCalls
            , "recent_advice" .= recentAdvice
            ]))
          packet
      case answer of
        Left err -> pure (Abstained (jevFailureSummary err))
        Right response -> do
          let a = J.answers response
              nudgeAnswers = NudgeAnswers
                { l0 = L0 (a.kind.key) (a.phase.key) (a.artifact.key)
                , stagesEverything = a.stages_everything.yes
                , doneWithStubs = a.done_with_stubs.yes
                , unwrapInLib = a.unwrap_in_lib.yes
                , stringlyTyped = a.stringly_typed.yes
                , blockingInAsync = a.blocking_in_async.yes
                , unboundedChannel = a.unbounded_channel.yes
                , orphanTask = a.orphan_task.yes
                , secretInOutput = a.secret_in_output.yes
                , hardcodedLimit = a.hardcoded_limit.yes
                , helperShipped = a.helper_shipped.yes
                , noncanonicalJson = a.noncanonical_json.yes
                , reinventsLibrary = a.reinvents_library.yes
                , repeatingItself = a.repeating_itself.yes
                , ignoringAFailure = a.ignoring_a_failure.yes
                , asksAnsweredQuestion = a.asks_answered_question.yes
                , askShapeBad = a.ask_shape.yes
                , investedScope = a.invented_scope.yes
                , interviewMissing = a.interview_missing.yes
                , destructiveCleared =
                    case J.settle J.strict a.destructive
                           ( #yes (\_ -> True) J..| #no (\_ -> False) ) of
                      Right (J.Settled cleared) -> cleared
                      Left _ -> False
                }
              disposition = handleAnswers nudgeAnswers recentAdvice
          writeLedger context call result nudgeAnswers disposition
          case disposition of
            DAbstained reason -> pure (Abstained reason)
            DAnnotated line escalated -> do
              if null escalated
                then pure ()
                else do
                  target <- parentAgent
                  case target of
                    Just parent -> void (sendMessage parent (escalationNote context call))
                    Nothing -> pure ()
              pure (Annotated line)

jevFailureSummary :: J.JevError -> Text
jevFailureSummary err =
  case err of
    J.Prepare _ -> "Jev request preparation failed; nudge left the result unchanged"
    J.Transport _ -> "Jev transport failed; nudge left the result unchanged"
    J.Decode _ -> "Jev response decoding failed; nudge left the result unchanged"

escalationNote :: ActorContextInfo -> ToolCall -> Text
escalationNote context call =
  "nudge escalation from " <> contextActorPath context
    <> " (actor " <> T.pack (show (contextActorId context)) <> "@" <> T.pack (show (contextActorIncarnation context)) <> ")"
    <> " on " <> toolCallName call <> ": destructive_command"

-- ---------------------------------------------------------------------------
-- Ledger: one entry per call, via this project's own 'Journal' effect
-- (already used by 'Project.FieldNotes' in the vendored workspace), named
-- per label so a label's history is addressable on its own -- the doc's
-- @.exomonad/nudges/<label>.jsonl@ intent, carried by the one durable-log
-- mechanism this project has rather than a second one.
-- ---------------------------------------------------------------------------

ledgerJson :: Text -> ToolCall -> NudgeAnswers -> Disposition -> Value
ledgerJson label call a disposition = object
  [ "label" .= label
  , "kind" .= l0Kind (l0 a)
  , "phase" .= l0Phase (l0 a)
  , "artifact" .= l0Artifact (l0 a)
  , "tool" .= toolCallName call
  , "shown" .= shownLine
  , "escalated" .= escalated
  ]
  where
    (shownLine, escalated) = case disposition of
      DAbstained reason -> (reason, False)
      DAnnotated line esc -> (line, not (null esc))

writeLedger
  :: Member Journal effects
  => ActorContextInfo -> ToolCall -> ToolResult -> NudgeAnswers -> Disposition -> Eff effects ()
writeLedger context call result a disposition =
  let label = contextActorPath context
  in record ("nudges:" <> label) (toolResultHandle result) (ledgerJson label call a disposition)

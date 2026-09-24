{-# LANGUAGE FlexibleContexts #-}
{-# LANGUAGE OverloadedStrings #-}

-- | Replay tests for 'Project.Nudges': a recorded ledger row (a
-- 'Nudges.NudgeAnswers' snapshot, exactly the shape 'Project.Nudges.watch'
-- builds from a live Jev response and writes to the Journal ledger) fed
-- through the pure handler, with no Jev call and no actor. This is the
-- "ledger replays => the handler is a pure test" the doc asks for.
module Project.NudgesChecks (replay) where

import Control.Monad.Freer (Eff, Member)
import qualified Data.Text as Text
import Tidepool.Check
import qualified Project.Nudges as Nudges

replay :: Member RecipeCheck effects => Eff effects ()
replay = do
  -- A recorded row: a `reply` call, in the `deliver` phase, on rust_lib,
  -- with a stub left behind. `done_with_stubs` is the only candidate.
  let stubRow = Nudges.NudgeAnswers
        { Nudges.l0 = Nudges.L0 "reply" "deliver" "rust_lib"
        , Nudges.stagesEverything = 0.1
        , Nudges.doneWithStubs = 0.9
        , Nudges.unwrapInLib = 0.2
        , Nudges.stringlyTyped = 0.1
        , Nudges.blockingInAsync = 0.0
        , Nudges.unboundedChannel = 0.0
        , Nudges.orphanTask = 0.0
        , Nudges.secretInOutput = 0.0
        , Nudges.hardcodedLimit = 0.0
        , Nudges.helperShipped = 0.0
        , Nudges.noncanonicalJson = 0.0
        , Nudges.reinventsLibrary = 0.0
        , Nudges.repeatingItself = 0.1
        , Nudges.ignoringAFailure = 0.1
        , Nudges.asksAnsweredQuestion = 0.0
        , Nudges.askShapeBad = 0.0
        , Nudges.investedScope = 0.0
        , Nudges.interviewMissing = 0.0
        , Nudges.destructiveCleared = False
        }

  case Nudges.handleAnswers stubRow [] of
    Nudges.DAnnotated line escalated ->
      check "a stub left in a reply is shown, with no escalation"
        ("finish or state exactly what is left" `Text.isInfixOf` line && null escalated)
    Nudges.DAbstained reason ->
      check ("expected an annotation for a stubbed reply, got abstained: " <> reason) False

  -- Replaying the same row with that advice already in `recent_advice`
  -- (as the ledger would show it from the previous call): anti-nag drops it.
  case Nudges.handleAnswers stubRow ["finish or state exactly what is left"] of
    Nudges.DAbstained _ -> check "anti-nag drops a repeat of the same advice" True
    Nudges.DAnnotated line _ ->
      check ("anti-nag failed to drop a repeated candidate: " <> line) False

  -- `kind = read` never annotates, even when a noul would otherwise trip.
  let readRow = stubRow { Nudges.l0 = Nudges.L0 "read" "orient" "nothing" }
  case Nudges.handleAnswers readRow [] of
    Nudges.DAbstained _ -> check "kind = read abstains even when a noul would trip" True
    Nudges.DAnnotated line _ ->
      check ("kind = read must never annotate: " <> line) False

  -- A destructive choice that clears strict settling escalates, even with
  -- nothing else to show.
  let destructiveRow = stubRow
        { Nudges.l0 = Nudges.L0 "check" "implement" "nothing"
        , Nudges.doneWithStubs = 0.0
        , Nudges.destructiveCleared = True
        }
  case Nudges.handleAnswers destructiveRow [] of
    Nudges.DAnnotated _ escalated ->
      check "a cleared destructive choice escalates" (escalated == ["destructive_command"])
    Nudges.DAbstained reason ->
      check ("expected an escalation for a cleared destructive choice, got abstained: " <> reason) False

  -- An unsettled (not cleared) destructive choice degrades to no
  -- escalation, per the doc: "otherwise the escalation noul degrades to
  -- advice" -- here, with nothing else tripped, to a plain abstention.
  let unsettledRow = destructiveRow { Nudges.destructiveCleared = False }
  case Nudges.handleAnswers unsettledRow [] of
    Nudges.DAbstained _ -> check "an unsettled destructive choice never escalates" True
    Nudges.DAnnotated _ escalated ->
      check "an unsettled destructive choice never escalates" (null escalated)

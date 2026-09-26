{-# LANGUAGE FlexibleContexts #-}

module SessionHelpers.Wave16Gate
  ( startWave16Gate, readWave16Gate
  , GateStart (..), CheckEntry (..), CheckOutcome (..)
  ) where

import Control.Monad.Freer (Eff, Member)
import Data.Text (Text)
import Project.FocusedGateExample
import Project.CheckResults (CheckEntry (..), CheckOutcome (..))
import qualified Tidepool.Actor.Record as R
import Tidepool.Actors.Exomonad (AgentRef)
import qualified Tidepool.Command as Cmd
import Tidepool.Effects.Core (Actor, Commands)
import Project.TestEvidence (FocusedSpec)
import Project.CheckResults (CheckActor)

-- One caller-owned check, with its source and expected count in FocusedSpec.
startWave16Gate
  :: (Member Actor effects, Member Commands effects)
  => AgentRef -> Text -> FocusedSpec -> Eff effects GateStart
startWave16Gate owner name = startGate owner name (Cmd.GiB 3)

-- Call on the terminal notice; callbacks handle failure and unknown evidence.
readWave16Gate
  :: Member Actor effects
  => R.ActorHandle CheckActor
  -> (CheckEntry -> CheckOutcome -> Eff effects ())
  -> (CheckEntry -> CheckOutcome -> Eff effects ())
  -> Eff effects (Maybe Text)
readWave16Gate = readGate

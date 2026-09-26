{-# LANGUAGE FlexibleContexts #-}
{-# LANGUAGE OverloadedLabels #-}
{-# LANGUAGE OverloadedRecordDot #-}
{-# LANGUAGE OverloadedStrings #-}
{-# LANGUAGE QuasiQuotes #-}
{-# LANGUAGE TypeApplications #-}

-- | A session seed to customize for one component's focused Cargo checks.
module SessionHelpers.TestEvidence
  ( FocusedSpec (..), FocusedRun (..), FocusedRecord (..), FailureKind (..)
  , FocusedResult (..), startFocused, finishFocused, focusedPassed
  ) where

import Control.Monad.Freer (Eff, Member)
import Data.Text (Text)
import qualified Data.Text as Text
import qualified Jev.Operators as J
import Jev.Operators (Packet ((:=), (:&)))
import qualified Tidepool.Command as Cmd
import Tidepool.Aeson (FromJSON (..), (.:), (.:?), withObject)
import Tidepool.Effects.Core (Commands, Jev)
import Tidepool.QQ.Bash (bash)

data FocusedSpec = FocusedSpec
  { focusedIntent :: Text
  , focusedSource :: Text
  , focusedPackage :: Text
  , focusedTarget :: Text
  , focusedFilter :: Text
  , focusedExpected :: Int
  } deriving (Show, Eq)

data FocusedRun = FocusedRun FocusedSpec Cmd.Job deriving (Show)

-- The runner writes these fields to evidence.json before test execution and
-- fills counts as they become known. Missing fields stay missing evidence.
data FocusedRecord = FocusedRecord
  { recordSource :: Maybe Text
  , recordWorkingTree :: Maybe Text
  , recordExecutable :: Text
  , recordDigest :: Text
  , recordOutput :: Text
  , recordRunnable :: Maybe [Text]
  , recordSummaries :: Maybe [[Int]]
  , recordExitCode :: Maybe Int
  } deriving (Show, Eq)

instance FromJSON FocusedRecord where
  parseJSON = withObject "focused test evidence" $ \fields ->
    FocusedRecord <$> fields .: "source"
      <*> fields .: "working_tree_status"
      <*> fields .: "executable"
      <*> fields .: "sha256"
      <*> fields .: "output"
      <*> fields .:? "runnable"
      <*> fields .:? "summaries"
      <*> fields .:? "exit_code"

data FailureKind
  = ImplementationFailure
  | FixtureFailure
  | MissingPrerequisite
  | InsufficientEvidence
  deriving (Show, Eq)

data FocusedResult = FocusedResult
  { focusedCommand :: Cmd.RunResult
  , focusedEvidencePath :: Maybe Text
  , focusedEvidence :: Either Text FocusedRecord
  , focusedFailure :: Maybe (Either Text FailureKind)
  } deriving (Show)

-- Run in the actor's checkout. The background job retains its source; wait on
-- Cmd.awaitFinished before calling finishFocused, then inspect its report too.
startFocused :: Member Commands effects => FocusedSpec -> Eff effects FocusedRun
startFocused spec = do
  job <- Cmd.background $ Cmd.withMemory (Cmd.GiB 4) $
    Cmd.withArguments
      [ focusedPackage spec, focusedTarget spec, focusedFilter spec
      , Text.pack (show (focusedExpected spec)) ]
      [bash|set -euo pipefail
scripts/cargo-focused-test --package "$1" --target "$2" --filter "$3" --expect "$4"|]
  pure (FocusedRun spec job)

finishFocused
  :: (Member Commands effects, Member Jev effects)
  => FocusedRun -> Eff effects FocusedResult
finishFocused (FocusedRun spec job) = do
  completed <- Cmd.await job
  let path = evidencePath (Cmd.stderr completed)
  evidence <- case path of
    Nothing -> pure (Left "focused runner did not report an evidence.json path")
    Just file -> do
      loaded <- Cmd.run (Cmd.argv ["cat", file])
      pure $ case Cmd.decodeWith (Cmd.asJSON @FocusedRecord) (Cmd.stdout loaded) of
        Left issue -> Left ("cannot read focused evidence: " <> Text.pack (show issue))
        Right record -> Right record
  judgment <- case (Cmd.failure completed, evidence) of
    (Nothing, _) -> pure Nothing
    (_, Left _) -> pure Nothing
    (Just _, Right record) -> do
      excerpt <- Cmd.run (Cmd.argv ["tail", "-n", "80", recordOutput record])
      let diagnostic = Text.takeEnd 8000 $ Cmd.stderr completed <> "\n" <>
            either (const "output log unavailable") id (Cmd.stdout excerpt)
      answer <- J.ask1
        (J.state (#intent := focusedIntent spec :& #diagnostic := diagnostic))
        (J.choice "Which explanation best fits this failed focused check?"
          (J.alt #implementation "The assertion or compiler diagnostic points to the implementation" ImplementationFailure
            J..| J.alt #fixture "The failure points to test setup or fixture data" FixtureFailure
            J..| J.alt #prerequisite "A missing tool, dependency or environment condition prevented the check" MissingPrerequisite
            J..| J.alt #insufficient "The retained diagnostic does not establish any of those causes" InsufficientEvidence))
      pure $ Just $ case answer of
        Left issue -> Left (Text.pack (show issue))
        Right choice -> case J.takenUnder J.lenient choice of
          Left doubt -> Left doubt.why
          Right (J.Settled kind) -> Right kind
  pure (FocusedResult completed path evidence judgment)

-- Exit, checkout identity, selected count and executed count are code facts.
-- A Jev classification is never an input to this predicate.
focusedPassed :: FocusedSpec -> FocusedResult -> Bool
focusedPassed spec result = case focusedEvidence result of
  Left _ -> False
  Right record ->
    Cmd.failure (focusedCommand result) == Nothing
      && recordSource record == Just (focusedSource spec)
      && recordWorkingTree record == Just ""
      && recordExitCode record == Just 0
      && maybe False ((== focusedExpected spec) . length) (recordRunnable record)
      && case recordSummaries record of
        Just [[passed, failed, _, _, _]] -> passed == focusedExpected spec && failed == 0
        _ -> False

evidencePath :: Text -> Maybe Text
evidencePath stderr = case
  [ Text.strip (Text.drop (Text.length marker) line)
  | line <- Text.lines stderr, marker `Text.isPrefixOf` line ] of
    path : _ | not (Text.null path) -> Just path
    _ -> Nothing
  where marker = "focused test evidence: "

{-# LANGUAGE DuplicateRecordFields #-}
{-# LANGUAGE OverloadedRecordDot #-}
{-# LANGUAGE TemplateHaskell #-}
{-# LANGUAGE NoFieldSelectors #-}
{-# OPTIONS_GHC -fconstraint-solver-iterations=10 #-}
{-# OPTIONS_GHC -fplugin=Protocols.Plugin #-}

module ClashIlaDemo where

import Clash.Annotations.TH
import Clash.Cores.UART (ValidBaud)
import Clash.Prelude

import Clash.Ila.Configurator
import Clash.Ila

import Domain
import Protocols

-- | Simple UART ILA demonstration
topLogicUart ::
  forall dom baud.
  (HiddenClockResetEnable dom, ValidBaud dom baud) =>
  -- | Baud rate of UART
  SNat baud ->
  -- | RX
  Signal dom Bit ->
  -- | Leds
  Signal dom Bit
topLogicUart baud rx = tx
 where
  -- Simple demo signal to 'debug'
  counter0 :: (HiddenClockResetEnable dom) => Signal dom (Unsigned 36)
  counter0 = register 0 $ satAdd SatWrap 1 <$> counter0
  counter1 :: (HiddenClockResetEnable dom) => Signal dom (Signed 12)
  counter1 = register 20 $ satAdd SatWrap 4 <$> counter1
  counter2 :: (HiddenClockResetEnable dom) => Signal dom (Signed 50)
  counter2 = register 40 $ satAdd SatWrap 3 <$> counter2

  Circuit demoIla = ilaUart
    baud
    $ ilaConfig
      -- Provide as many signals (& their names) you would like to debug
      (counter0, "+1")
      (counter1, "+4")
      (counter2, "+3")
      -- Finalize the ILA by giving it a configuration
      WithIlaConfig
        { bufferDepth = d100
        -- ^ Amount of samples to store
        , name = "Simple_Demo"
        -- ^ The name displayed in the waveform viewer
        , triggerPoint = 0
        -- ^ Amount of samples in the buffer after trigger
        , predicates = ilaDefaultPredicates
        -- ^ The list of predicates to select from during runtime
        }
  tx = snd $ demoIla (rx, (()))

-- | The top entity
topEntity ::
  "CLK" ::: Clock Dom48 ->
  "BTN" ::: Reset Dom48 ->
  "PMOD1_6" ::: Signal Dom48 Bit ->
  "PMOD1_5" ::: Signal Dom48 Bit
topEntity clk rst = withClockResetEnable clk rst enableGen (topLogicUart (SNat @115200))

makeTopEntity 'topEntity

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
import Data.Word (Word32)
import Data.Data (Proxy(..))

-- | Simple UART ILA demonstration
topLogicUart ::
  forall dom baud.
  (HiddenClockResetEnable dom, ValidBaud dom baud) =>
  -- | Baud rate of UART
  SNat baud ->
  -- | RX
  Signal dom Bit ->
  -- | TX and LED output
  ( Signal dom Bit
  , Signal dom Bool
  , Signal dom Bool
  , Signal dom Bool
  )
topLogicUart baud rx = (tx, not <$> red, not <$> green, not <$> blue)
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
        , outputs = ilaOutputs (Proxy @(Unsigned 29), "foo") (Proxy @Bool, "red") (Proxy @Bool, "green") (Proxy @Bool, "blue") :: (Vec 4 GenSignal, Proxy (((((), (Unsigned 29)), Bool), Bool), Bool))
        }
  (tx, outputs) = snd $ demoIla (rx, ((),()))
  (red, green, blue) = unbundle $ (\(((((), _), r), g), b) -> (r, g, b)) <$> outputs

-- | The top entity
topEntity ::
  "CLK" ::: Clock Dom48 ->
  "BTN" ::: Reset Dom48 ->
  "PMOD1_6" ::: Signal Dom48 Bit ->
  ( "PMOD1_5" ::: Signal Dom48 Bit
  , "rgb_led0_r" ::: Signal Dom48 Bool
  , "rgb_led0_g" ::: Signal Dom48 Bool
  , "rgb_led0_b" ::: Signal Dom48 Bool
  )
topEntity clk rst = withClockResetEnable clk rst enableGen (topLogicUart (SNat @115200))

makeTopEntity 'topEntity

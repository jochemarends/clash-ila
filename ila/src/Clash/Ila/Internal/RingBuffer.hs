{-# LANGUAGE OverloadedRecordDot #-}
{-# LANGUAGE DuplicateRecordFields #-}
{-# LANGUAGE NoFieldSelectors #-}

module Clash.Ila.Internal.RingBuffer where

import Clash.Prelude

import Protocols
import Protocols.PacketStream

{- | A non-conventional ring buffer, capable of writing data to the tail of the buffer, overwriting old data
Can read from any point within the buffer specified by the index
-}
ringBuffer ::
  forall dom size a.
  (HiddenClockResetEnable dom, NFDataX a, KnownNat size, 1 <= size) =>
  -- | The size of the circle buffer
  SNat size ->
  -- | Initial value to use with
  a ->
  -- | Clear the buffer
  -- NOTE: any writes during a clear cycle will be discarded
  -- NOTE: a read prior to the clear cycle will fail!
  Signal dom Bool ->
  -- | Value to push onto the buffer
  Signal dom (Maybe a) ->
  -- | Index in the circle buffer to read from
  Signal dom (Index size) ->
  -- | Value stored at the index specified
  -- NOTE: Reads are delayed by 1 cycle
  (Signal dom a, Signal dom (Index (size + 1)))
ringBuffer size ini clear writeData readIndex =
  (blockRam1 NoClearOnReset size ini readAddr ramWrite, bufferSize)
 where
  headTail :: Signal dom (Index size, Index size)
  headTail =
    register (0, 0)
      $ mux
        clear
        (pure (0, 0))
      $ liftA3 newHeadTail headTail writeData bufferSize

  bufferSize :: Signal dom (Index (size + 1))
  bufferSize =
    register 0
      $ mux
        clear
        (pure 0)
      $ newLength
      <$> bundle (bufferSize, writeData)

  newLength (old, Nothing) = old
  newLength (old, Just _) = satAdd SatBound old 1

  newHeadTail old Nothing _ = old
  newHeadTail (_, oldTail) (Just _) len = (newHead, newTail)
   where
    newTail = satAdd SatWrap oldTail 1

    newHead
      -- If we're overflowing, move the head to match the tail
      -- Otherwise, be zero
      | len == maxBound = newTail
      | otherwise = 0

  readAddr :: Signal dom (Index size)
  readAddr = liftA2 (satAdd SatWrap) (fst $ unbundle headTail) readIndex

  ramWrite :: Signal dom (Maybe (Index size, a))
  ramWrite = liftA2 (\addr write -> (fmap (addr,) write)) (snd $ unbundle headTail) writeData

{- | Reads out the entire buffer using the PacketStream protocol
The data will be chopped up in bytes and sent one-by-one each clock cycle
-}
ringBufferReaderPS ::
  forall dom size dat.
  ( HiddenClockResetEnable dom
  , NFDataX dat
  , BitPack dat
  , 1 <= size
  , KnownNat size
  ) =>
  -- | The buffer component
  (Signal dom (Index size) -> (Signal dom dat, Signal dom (Index (size + 1)))) ->
  -- | The reader circuit, whilst the input is high, it will attempt reading out data from the buffer
  -- and sends that data via a `PacketStream`. If input is any point low, it will read from the beginning again
  Circuit
    (CSignal dom Bool)
    (PacketStream dom (BitSize dat `DivRU` 8) (Index (size + 1)))
ringBufferReaderPS buffer = Circuit exposeIn
 where
  exposeIn (active, s2m) = out
   where
    doIncrement = active .&&. _ready <$> s2m

    (bufferValue, bufferLength) = buffer index

    -- \| Sync the activity with the delay from the RAM read
    delayedActive = register False active
    -- \| The output value is always invalid (thus should be Nothing) if we already wrote all bytes
    -- or the buffer has no content
    valueValid = delayedActive .&&. (bufferLength ./=. pure 0) .&&. readCount ./=. bufferLength

    -- Deliberately delay the read count by one clock cycle to sync with the actual read from RAM
    readCount =
      register 0
        $ mux
          active
          (mux doIncrement (satAdd SatBound 1 <$> readCount) readCount)
          0

    -- Incrementing this way gets us the new index *immediately* rather than having to wait a cycle
    -- for the register to update
    oldIndex :: Signal dom (Index size)
    oldIndex = register 0 index
    index :: Signal dom (Index size)
    index =
      mux
        active
        (mux doIncrement (satAdd SatBound 1 <$> oldIndex) oldIndex)
        0

    -- \| Takes in any type (as long as it can be `BitPack`'d and chops it up into a vector of bytes
    splitIntoBytes :: (BitPack a) => a -> Vec (BitSize a `DivRU` 8) (BitVector 8)
    splitIntoBytes bv = unpack . resize $ pack bv

    -- \| Formats the output data into the PacketStream format
    -- Once we reached the last byte, _last should be valid for all bytes considering we send data
    -- one-by-one anyways
    formatData value len count =
      PacketStreamM2S
        { _data = splitIntoBytes value
        , _last = if satSub SatZero len 1 == count then Just maxBound else Nothing
        , _meta = len
        , _abort = False
        }

    packetStream =
      mux
        valueValid
        (Just <$> liftA3 formatData bufferValue bufferLength readCount)
        (pure Nothing)

    out = ((), packetStream)

-- | A testbench for the ring buffer, capable of reading out the entire content of the ringBuffer
testbenchRingBuffer ::
  forall dom a size.
  (HiddenClockResetEnable dom, NFDataX a, KnownNat size, 1 <= size) =>
  -- | Every clock this is active, it will output one value from the buffer
  -- If all values are read, the output will be `Nothing`
  Signal dom Bool ->
  -- | The ring buffer component
  (Signal dom (Index size) -> (Signal dom a, Signal dom (Index (size + 1)))) ->
  -- | Data from the buffer, takes `size + 1` amount of cycles to read out the entire buffer
  -- The first set of data will arive 1 cycles after being triggered
  Signal dom (Maybe a)
testbenchRingBuffer active buffer = readValue
 where
  (bufferValue, bufferLength) = buffer index

  -- \| Sync the activity with the delay from the RAM read
  delayedActive = register False active
  -- \| The output value is always invalid (thus should be Nothing) if we already wrote all bytes
  -- or the buffer has no content
  valueValid = delayedActive .&&. (bufferLength ./=. pure 0) .&&. writeCount ./=. bufferLength

  -- \| Keep track of how many items we read out
  writeCount :: Signal dom (Index (size + 1))
  writeCount = register 0 $ liftA2 newWriteCount valueValid writeCount

  newWriteCount :: Bool -> Index (size + 1) -> Index (size + 1)
  newWriteCount True count = count + 1
  newWriteCount False count = count

  index = register 0 $ liftA3 newIndex active index bufferLength

  newIndex True i len
    | i == (resize $ satSub SatZero len 1) = i
    | otherwise = i + 1
  newIndex False i _ = i

  readValue =
    mux
      valueValid
      (Just <$> bufferValue)
      (pure Nothing)

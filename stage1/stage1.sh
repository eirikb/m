if [ -f ./.cache/gg/stage2.sh ]; then
  ./.cache/gg/stage2.sh "$@"
  exit
fi

tail -c +434 gg.cmd | tar -zpx && ./.cache/gg/stage2.sh "$@"
exit
